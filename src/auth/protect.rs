//! Current-user Windows DPAPI protection for the on-disk PAT cache.
use anyhow::Result;
use windows::{
    Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    },
    core::PCWSTR,
};

struct ProtectedBuffer(CRYPT_INTEGER_BLOB);
impl Drop for ProtectedBuffer {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            // SAFETY: DPAPI returned this buffer with cbData initialized bytes allocated
            // by LocalAlloc. The guard exclusively owns it until LocalFree below.
            unsafe {
                std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).fill(0);
                let _ = LocalFree(Some(HLOCAL(self.0.pbData.cast())));
            }
        }
    }
}

pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
    transform(data, true)
}

pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    transform(data, false)
}

fn transform(data: &[u8], protect: bool) -> Result<Vec<u8>> {
    let mut input = data.to_vec();
    let blob = CRYPT_INTEGER_BLOB {
        cbData: input.len().try_into()?,
        pbData: input.as_mut_ptr(),
    };
    let mut output = ProtectedBuffer(CRYPT_INTEGER_BLOB::default());
    // SAFETY: All blobs and their backing buffers live through this synchronous call.
    // Optional pointers are null; UI is forbidden. DPAPI allocates the output, which
    // the guard frees. No machine-wide flag is set: protection is for this user only.
    let result = unsafe {
        if protect {
            CryptProtectData(
                &blob,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        } else {
            CryptUnprotectData(
                &blob,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        }
    };
    input.fill(0);
    result.map_err(|_| anyhow::anyhow!("Cannot protect or read the saved PAT with Windows DPAPI. Run `devops login` to replace it."))?;
    anyhow::ensure!(
        !output.0.pbData.is_null() && output.0.cbData > 0,
        "Windows DPAPI returned no data"
    );
    // SAFETY: Successful DPAPI call returned cbData bytes owned by output; copy them
    // before the guard wipes and frees the allocation.
    Ok(unsafe { std::slice::from_raw_parts(output.0.pbData, output.0.cbData as usize) }.to_vec())
}
