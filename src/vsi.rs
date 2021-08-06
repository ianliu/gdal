use std::ffi::CString;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;

use gdal_sys::{VSIFCloseL, VSIFileFromMemBuffer, VSIFree, VSIGetMemFileBuffer, VSIUnlink};

use crate::errors::{GdalError, Result};
use crate::utils::_last_null_pointer_err;

/// Creates a new VSIMemFile from a given buffer.
pub fn create_mem_file(file_name: &str, data: Vec<u8>) -> Result<()> {
    let file_name = CString::new(file_name)?;

    // ownership will be given to GDAL, so it should not be automaticly dropped
    let mut data = ManuallyDrop::new(data);

    let handle = unsafe {
        VSIFileFromMemBuffer(
            file_name.as_ptr(),
            data.as_mut_ptr(),
            data.len() as u64,
            true as i32,
        )
    };

    if handle.is_null() {
        // on error, allow dropping the data again
        ManuallyDrop::into_inner(data);
        return Err(_last_null_pointer_err("VSIGetMemFileBuffer"));
    }

    unsafe {
        VSIFCloseL(handle);
    }

    Ok(())
}

/// A helper struct that unlinks a mem file that points to borrowed data
/// before that data is freed.
pub struct MemFileRef<'d> {
    file_name: String,
    data_ref: PhantomData<&'d mut ()>,
}

impl<'d> MemFileRef<'d> {
    pub fn new(file_name: &str) -> MemFileRef<'d> {
        Self {
            file_name: file_name.to_string(),
            data_ref: PhantomData::default(),
        }
    }
}

impl<'d> Drop for MemFileRef<'d> {
    fn drop(&mut self) {
        // try to unlink file
        // if it fails, ignore - it probably was manually unlinked before
        let _ = unlink_mem_file(&self.file_name);
    }
}

/// Creates a new VSIMemFile from a given buffer reference.
/// Returns a handle that has a lifetime that is shorter than `data`.
pub fn create_mem_file_from_ref<'d>(file_name: &str, data: &'d mut [u8]) -> Result<MemFileRef<'d>> {
    let file_name_c = CString::new(file_name)?;

    let handle = unsafe {
        VSIFileFromMemBuffer(
            file_name_c.as_ptr(),
            data.as_mut_ptr(),
            data.len() as u64,
            false as i32,
        )
    };

    if handle.is_null() {
        return Err(_last_null_pointer_err("VSIGetMemFileBuffer"));
    }

    unsafe {
        VSIFCloseL(handle);
    }

    Ok(MemFileRef::new(file_name))
}

/// Unlink a VSIMemFile.
pub fn unlink_mem_file(file_name: &str) -> Result<()> {
    let file_name_c = CString::new(file_name)?;

    let rv = unsafe { VSIUnlink(file_name_c.as_ptr()) };

    if rv != 0 {
        return Err(GdalError::UnlinkMemFile {
            file_name: file_name.to_string(),
        });
    }

    Ok(())
}

/// Copies the bytes of the VSIMemFile with given `file_name`.
/// Takes the ownership and frees the memory of the VSIMemFile.
pub fn get_vsi_mem_file_bytes_owned(file_name: &str) -> Result<Vec<u8>> {
    let file_name = CString::new(file_name)?;

    let owned_bytes = unsafe {
        let mut length: u64 = 0;
        let bytes = VSIGetMemFileBuffer(file_name.as_ptr(), &mut length, true as i32);

        if bytes.is_null() {
            return Err(_last_null_pointer_err("VSIGetMemFileBuffer"));
        }

        let slice = std::slice::from_raw_parts(bytes, length as usize);
        let vec = slice.to_vec();

        VSIFree(bytes.cast::<std::ffi::c_void>());

        vec
    };

    Ok(owned_bytes)
}

/// Computes a function on the bytes of the vsi in-memory file with given `file_name`.
/// This method is useful if you don't want to take the ownership of the memory.
pub fn call_on_mem_file_bytes<F, R>(file_name: &str, fun: F) -> Result<R>
where
    F: FnOnce(&[u8]) -> R,
{
    let file_name = CString::new(file_name)?;

    unsafe {
        let mut length: u64 = 0;
        let bytes = VSIGetMemFileBuffer(file_name.as_ptr(), &mut length, false as i32);

        if bytes.is_null() {
            return Err(_last_null_pointer_err("VSIGetMemFileBuffer"));
        }

        let slice = std::slice::from_raw_parts(bytes, length as usize);

        Ok(fun(slice))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_retrieve_mem_file() {
        let file_name = "/vsimem/525ebf24-a030-4677-bb4e-a921741cabe0";

        create_mem_file(file_name, vec![1_u8, 2, 3, 4]).unwrap();

        let bytes = get_vsi_mem_file_bytes_owned(file_name).unwrap();

        assert_eq!(bytes, vec![1_u8, 2, 3, 4]);

        // mem file must not be there anymore
        assert_eq!(
            unlink_mem_file(file_name),
            Err(GdalError::UnlinkMemFile {
                file_name: file_name.to_string()
            })
        );
    }

    #[test]
    fn create_and_callmem_file() {
        let file_name = "/vsimem/ee08caf2-a510-4b21-a4c4-44c1ebd763c8";

        create_mem_file(file_name, vec![1_u8, 2, 3, 4]).unwrap();

        let result = call_on_mem_file_bytes(file_name, |bytes| {
            bytes.iter().map(|b| b * 2).collect::<Vec<u8>>()
        })
        .unwrap();

        assert_eq!(result, vec![2_u8, 4, 6, 8]);

        unlink_mem_file(file_name).unwrap();
    }

    #[test]
    fn create_and_unlink_mem_file() {
        let file_name = "/vsimem/bbf5f1d6-c1e9-4469-a33b-02cd9173132d";

        create_mem_file(file_name, vec![1_u8, 2, 3, 4]).unwrap();

        unlink_mem_file(file_name).unwrap();
    }

    #[test]
    fn no_mem_file() {
        assert_eq!(
            get_vsi_mem_file_bytes_owned("foobar"),
            Err(GdalError::NullPointer {
                method_name: "VSIGetMemFileBuffer",
                msg: "".to_string(),
            })
        );
    }

    #[test]
    fn create_and_unlink_mem_file_from_ref() {
        let file_name = "/vsimem/58e61d06-c96b-4ac0-9dd5-c37f69508454";

        let mut data = vec![1_u8, 2, 3, 4];

        let ref_handle = create_mem_file_from_ref(file_name, &mut data).unwrap();

        drop(ref_handle);

        // data was not corrupted
        assert_eq!(data, vec![1_u8, 2, 3, 4]);
    }

    #[test]
    fn mem_file_ref_double_unlink() {
        let file_name = "/vsimem/86df94a7-051d-4582-b141-d705ba8d8e83";

        let mut data = vec![1_u8, 2, 3, 4];

        let ref_handle = create_mem_file_from_ref(file_name, &mut data).unwrap();

        unlink_mem_file(file_name).unwrap();

        drop(ref_handle);
    }

    #[test]
    fn unable_to_create() {
        let file_name = "";

        assert_eq!(
            create_mem_file(file_name, vec![1_u8, 2, 3, 4]),
            Err(GdalError::NullPointer {
                method_name: "VSIGetMemFileBuffer",
                msg: "".to_string(),
            })
        );

        assert_eq!(
            unlink_mem_file(file_name),
            Err(GdalError::UnlinkMemFile {
                file_name: "".to_string()
            })
        );
    }
}