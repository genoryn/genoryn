//! Genoryn Krypton integration SDK.
//!
//! * Rust: re-exports of the stable building blocks (addresses, amounts, the
//!   verifying light client, the wallet engine).
//! * C: a small, stable ABI (`include/krypton.h`) for exchanges, payment
//!   processors and wallets written in other languages.

pub use krypton_core::address::Address;
pub use krypton_core::emission::{COIN, MAX_SUPPLY, format_amount, parse_amount, subsidy};
pub use krypton_core::{Network, Transaction};
pub use krypton_light::client::{NodeClient, NodeInfo};
pub use krypton_light::{CompactBlock, CompactVerifier, verify_flyclient};
pub use krypton_wallet_core::{Balance, ChainSource, Wallet};

/// Validates an address string; returns its network.
pub fn validate_address(s: &str) -> Option<Network> {
    Address::decode(s).ok().map(|a| a.network)
}

pub mod ffi {
    //! C ABI. Strings are NUL-terminated UTF-8; output buffers are caller-owned.
    #![allow(unsafe_code)]
    use super::*;
    use std::ffi::{CStr, c_char, c_int};

    unsafe fn input<'a>(p: *const c_char) -> Option<&'a str> {
        if p.is_null() {
            return None;
        }
        // SAFETY: the caller passes a valid NUL-terminated string (documented in krypton.h).
        unsafe { CStr::from_ptr(p) }.to_str().ok()
    }

    fn write_out(s: &str, out: *mut c_char, cap: usize) -> c_int {
        let b = s.as_bytes();
        if out.is_null() || cap < b.len() + 1 {
            return -((b.len() + 1) as c_int);
        }
        // SAFETY: `out` has at least `cap` bytes (caller contract), checked above.
        unsafe {
            std::ptr::copy_nonoverlapping(b.as_ptr(), out as *mut u8, b.len());
            *out.add(b.len()) = 0;
        }
        b.len() as c_int
    }

    /// Library version string (static, do not free).
    #[unsafe(no_mangle)]
    pub extern "C" fn krypton_version() -> *const c_char {
        concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
    }

    /// 1 = mainnet, 2 = testnet, 3 = devnet, 0 = invalid address.
    ///
    /// # Safety
    /// ddr must be null or a valid NUL-terminated string.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn krypton_address_network(addr: *const c_char) -> c_int {
        unsafe { input(addr) }
            .and_then(validate_address)
            .map(|n| n.id() as c_int)
            .unwrap_or(0)
    }

    /// Writes the short form of `addr`. Returns its length, or a negative required size.
    ///
    /// # Safety
    /// `addr` must be null or NUL-terminated; `out` must point to `cap` writable bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn krypton_short_address(
        addr: *const c_char,
        out: *mut c_char,
        cap: usize,
    ) -> c_int {
        match unsafe { input(addr) }.and_then(|s| Address::decode(s).ok()) {
            Some(a) => write_out(&a.short(), out, cap),
            None => 0,
        }
    }

    /// Parses "12.5" into base units (1 GNR = 100,000,000 gori). Returns 1 on success.
    ///
    /// # Safety
    /// `s` must be null or NUL-terminated; `out` must be null or point to a writable u64.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn krypton_parse_amount(s: *const c_char, out: *mut u64) -> c_int {
        match (unsafe { input(s) }.and_then(parse_amount), out.is_null()) {
            (Some(v), false) => {
                // SAFETY: non-null pointer to a u64 (caller contract).
                unsafe { *out = v };
                1
            }
            _ => 0,
        }
    }

    /// Formats base units as a decimal GNR string.
    ///
    /// # Safety
    /// `out` must point to `cap` writable bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn krypton_format_amount(
        gori: u64,
        out: *mut c_char,
        cap: usize,
    ) -> c_int {
        write_out(&format_amount(gori), out, cap)
    }

    /// Block subsidy at `height`, in base units.
    #[unsafe(no_mangle)]
    pub extern "C" fn krypton_block_subsidy(height: u64) -> u64 {
        subsidy(height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    #[test]
    fn ffi_amounts_and_addresses() {
        let mut v = 0u64;
        let s = CString::new("12.5").unwrap();
        assert_eq!(unsafe { ffi::krypton_parse_amount(s.as_ptr(), &mut v) }, 1);
        assert_eq!(v, 1_250_000_000);
        let mut buf = [0i8; 32];
        let n = unsafe { ffi::krypton_format_amount(v, buf.as_mut_ptr() as *mut _, buf.len()) };
        assert_eq!(n, 11);
        let bad = CString::new("tgnr1notanaddress").unwrap();
        assert_eq!(unsafe { ffi::krypton_address_network(bad.as_ptr()) }, 0);
        assert_eq!(ffi::krypton_block_subsidy(1), 5 * COIN);
    }
}
