use std::fs;
use std::io::Write;

unsafe fn sc(nr: i64, args: &[i64]) -> i64 {
    match args.len() {
        0 => libc::syscall(nr),
        1 => libc::syscall(nr, args[0]),
        2 => libc::syscall(nr, args[0], args[1]),
        3 => libc::syscall(nr, args[0], args[1], args[2]),
        4 => libc::syscall(nr, args[0], args[1], args[2], args[3]),
        5 => libc::syscall(nr, args[0], args[1], args[2], args[3], args[4]),
        6 => libc::syscall(nr, args[0], args[1], args[2], args[3], args[4], args[5]),
        _ => panic!("too many args"),
    }
}

fn main() {
    let path = "/root/enosys_out.txt";
    let mut f = fs::File::create(path).expect("create out file");

    writeln!(f, "=== Syscall ENOSYS Tester (aarch64) ===").unwrap();
    f.flush().unwrap();

    let tests: &[(&str, i64, &[i64])] = &[
        ("getrandom (278)", 278, &[0, 16, 0]),
        ("pipe2 (59)", 59, &[0, 0]),
        ("clone3 (435)", 435, &[0, 0]),
        ("statx (291)", 291, &[-100, 0, 0, 0xfff, 0]),
        ("preadv2 (286)", 286, &[0, 0, 0, 0, 0, 0]),
        ("copy_file_range (285)", 285, &[0, 0, 0, 0, 0]),
        ("process_madvise (440)", 440, &[0, 0, 0, 0, 0]),
        ("memfd_create (279)", 279, &[0, 1]),
        ("renameat2 (276)", 276, &[-100, 0, -100, 0, 0]),
        ("faccessat2 (439)", 439, &[-100, 0, libc::R_OK as i64, 0]),
        ("openat (56)", 56, &[-100, 0, libc::O_RDONLY as i64, 0]),
        ("fstatfs (44)", 44, &[0, 0]),
        ("clock_gettime (113)", 113, &[0, 0]),
        ("clock_gettime64 (403)", 403, &[0, 0]),
        ("rseq (293)", 293, &[0, 0, 0, 0]),
        ("setrlimit (164)", 164, &[0, 0]),
        ("prlimit64 (261)", 261, &[0, 0, 0, 0]),
        ("madvise (233)", 233, &[0, 0, 0]),
        ("close (57)", 57, &[-1]),
        ("dup3 (24)", 24, &[0, 0, 0]),
        ("fstat (80)", 80, &[0, 0]),
        ("lseek (62)", 62, &[0, 0, 0]),
        ("brk (214)", 214, &[0]),
        ("getpid (172)", 172, &[]),
    ];

    for (name, nr, args) in tests {
        let result = unsafe { sc(*nr, args) };
        if result == -1 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::ENOSYS {
                writeln!(f, "ENOSYS: {} (errno=38)", name).unwrap();
            } else {
                writeln!(f, "ERR({}): {}", errno, name).unwrap();
            }
        } else {
            writeln!(f, "OK: {} rc={}", name, result).unwrap();
        }
        f.flush().unwrap();
    }

    writeln!(f, "=== DONE ===").unwrap();
}

#[cfg(test)]
mod tests {
    use super::sc;

    #[test]
    fn sc_zero_arg_getpid_returns_non_negative() {
        let rc = unsafe { sc(libc::SYS_getpid, &[]) };
        assert!(rc > 0);
    }

    #[test]
    fn sc_panics_when_more_than_six_args_are_passed() {
        let result = std::panic::catch_unwind(|| unsafe {
            sc(libc::SYS_getpid, &[1, 2, 3, 4, 5, 6, 7]);
        });
        assert!(result.is_err());
    }
}
