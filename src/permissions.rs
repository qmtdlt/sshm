use anyhow::Result;
use std::path::Path;

#[cfg(unix)]
pub fn owner_only(path: &Path, directory: bool) -> Result<()> {
    use std::{fs, os::unix::fs::PermissionsExt};
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )?;
    Ok(())
}

#[cfg(windows)]
pub fn owner_only(path: &Path, directory: bool) -> Result<()> {
    use anyhow::ensure;
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
            SetFileSecurityW, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    struct Allocation(*mut c_void);
    impl Drop for Allocation {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    fn check(ok: i32) -> Result<()> {
        ensure!(
            ok != 0,
            "无法限制密钥文件权限：{}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }
    // All buffers stay alive until the associated Win32 call completes.
    // The DACL is protected from inheritance and grants only the current SID.
    unsafe {
        let mut handle = ptr::null_mut();
        check(OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY,
            &mut handle,
        ))?;
        let token = Token(handle);
        let mut length = 0;
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut length);
        ensure!(length > 0, "无法确定当前用户 SID");
        let mut buffer = vec![0_usize; (length as usize).div_ceil(size_of::<usize>())];
        check(GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        ))?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = ptr::null_mut();
        check(ConvertSidToStringSidW(user.User.Sid, &mut sid))?;
        let _sid_memory = Allocation(sid.cast());
        let mut count = 0;
        while *sid.add(count) != 0 {
            count += 1;
        }
        let sid = String::from_utf16(std::slice::from_raw_parts(sid, count))?;
        let inheritance = if directory { "OICI" } else { "" };
        let sddl: Vec<u16> = format!("D:P(A;{inheritance};FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        ))?;
        let _descriptor_memory = Allocation(descriptor);
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        check(SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        ))?;
    }
    Ok(())
}
