use std::env;
use std::process;
use std::time::Instant;

mod clone;
mod distro;
mod gcc;
mod general;
mod git;
mod pipe;
mod readlink;
mod rust;
mod ssh;

type TestResult = Result<(), String>;

struct Test {
    name: &'static str,
    run: fn() -> TestResult,
}

struct Suite {
    name: &'static str,
    probe: fn() -> bool,
    tests: &'static [Test],
}

struct TapReporter {
    passed: usize,
    failed: usize,
    skipped: usize,
    failures: Vec<String>,
}

impl TapReporter {
    fn new() -> Self {
        Self {
            passed: 0,
            failed: 0,
            skipped: 0,
            failures: Vec::new(),
        }
    }

    fn ok(&mut self, num: usize, name: &str) {
        self.passed += 1;
        println!("ok {} - {}", num, name);
    }

    fn not_ok(&mut self, num: usize, name: &str, detail: &str) {
        self.failed += 1;
        println!("not ok {} - {}", num, name);
        println!("# {}", detail);
        self.failures.push(name.to_string());
    }

    fn skip(&mut self, num: usize, name: &str, reason: &str) {
        self.skipped += 1;
        println!("ok {} - {} # SKIP {}", num, name, reason);
    }

    fn total(&self) -> usize {
        self.passed + self.failed + self.skipped
    }
}

const SUITES: &[Suite] = &[
    Suite {
        name: "distro",
        probe: distro::probe,
        tests: &[
            Test {
                name: "detect package manager",
                run: distro::test_detect_pm,
            },
            Test {
                name: "update repos",
                run: distro::test_update_repos,
            },
            Test {
                name: "install tools",
                run: distro::test_install_tools,
            },
            Test {
                name: "verify vim",
                run: distro::test_verify_vim,
            },
            Test {
                name: "verify gcc",
                run: distro::test_verify_gcc,
            },
            Test {
                name: "verify rustc",
                run: distro::test_verify_rustc,
            },
            Test {
                name: "verify cargo",
                run: distro::test_verify_cargo,
            },
            Test {
                name: "read /etc/os-release",
                run: distro::test_os_release,
            },
        ],
    },
    Suite {
        name: "clone",
        probe: clone::probe,
        tests: &[
            Test {
                name: "fork+exec baseline",
                run: clone::test_fork_exec,
            },
            Test {
                name: "Command stdout piped",
                run: clone::test_stdout_piped,
            },
            Test {
                name: "nested spawn",
                run: clone::test_nested_spawn,
            },
            Test {
                name: "CLONE_THREAD preserved",
                run: clone::test_thread,
            },
            Test {
                name: "concurrent spawn stress",
                run: clone::test_concurrent_spawn,
            },
        ],
    },
    Suite {
        name: "readlink",
        probe: readlink::probe,
        tests: &[
            Test {
                name: "regular symlink resolves",
                run: readlink::test_symlink_resolve,
            },
            Test {
                name: "realpath no .l2s.",
                run: readlink::test_realpath_no_l2s,
            },
            Test {
                name: "readlink EINVAL on .l2s.",
                run: readlink::test_readlink_einval,
            },
            Test {
                name: "/proc/self/exe no .l2s.",
                run: readlink::test_proc_self_exe,
            },
            Test {
                name: "lstat vs stat consistency",
                run: readlink::test_lstat_stat,
            },
            Test {
                name: "readlink small buffer",
                run: readlink::test_readlink_small_buffer,
            },
        ],
    },
    Suite {
        name: "gcc",
        probe: gcc::probe,
        tests: &[
            Test {
                name: "cc -print-search-dirs",
                run: gcc::test_search_dirs,
            },
            Test {
                name: "compile and run C program",
                run: gcc::test_compile_c,
            },
            Test {
                name: "/proc/self/exe after exec",
                run: gcc::test_proc_exe,
            },
        ],
    },
    Suite {
        name: "rust",
        probe: rust::probe,
        tests: &[
            Test {
                name: "rustc -vV",
                run: rust::test_rustc_version,
            },
            Test {
                name: "rustc compile .rs",
                run: rust::test_rustc_compile,
            },
            Test {
                name: "cargo build --vcs none",
                run: rust::test_cargo_no_vcs,
            },
            Test {
                name: "cargo build with git",
                run: rust::test_cargo_with_vcs,
            },
        ],
    },
    Suite {
        name: "git",
        probe: git::probe,
        tests: &[
            Test {
                name: "git init",
                run: git::test_git_init,
            },
            Test {
                name: "git config",
                run: git::test_git_config,
            },
            Test {
                name: "cargo new with vcs git",
                run: git::test_cargo_new_git,
            },
        ],
    },
    Suite {
        name: "pipe",
        probe: pipe::probe,
        tests: &[
            Test {
                name: "pipe() baseline",
                run: pipe::test_pipe_baseline,
            },
            Test {
                name: "pipe2(O_CLOEXEC)",
                run: pipe::test_pipe2_cloexec,
            },
            Test {
                name: "pipe2(O_NONBLOCK)",
                run: pipe::test_pipe2_nonblock,
            },
        ],
    },
    Suite {
        name: "general",
        probe: general::probe,
        tests: &[
            Test {
                name: "file I/O roundtrip",
                run: general::test_file_io,
            },
            Test {
                name: "symlink operations",
                run: general::test_symlink_ops,
            },
            Test {
                name: "pipe between processes",
                run: general::test_pipe,
            },
            Test {
                name: "signal propagation",
                run: general::test_signal,
            },
            Test {
                name: "environment inheritance",
                run: general::test_env,
            },
            Test {
                name: "touch creates a missing file",
                run: general::test_touch_creates_file,
            },
        ],
    },
    Suite {
        name: "ssh",
        probe: ssh::probe,
        tests: &[
            Test {
                name: "ssh -V (OpenSSH version)",
                run: ssh::test_ssh_version,
            },
            Test {
                name: "ssh-keygen ed25519 key generation",
                run: ssh::test_ssh_keygen_ed25519,
            },
            Test {
                name: "ssh-keygen SHA256 fingerprint",
                run: ssh::test_ssh_keygen_fingerprint,
            },
            Test {
                name: "scp available",
                run: ssh::test_scp_available,
            },
        ],
    },
];

fn run_suite(suite: &Suite, reporter: &mut TapReporter) {
    eprintln!("[* Running test suite: {}... {}", suite.name, now());

    if !(suite.probe)() {
        for test in suite.tests {
            reporter.skip(reporter.total() + 1, test.name, "prerequisites not met");
        }
        eprintln!("[* Suite {} done (skipped)]", suite.name);
        return;
    }

    let start = Instant::now();
    for test in suite.tests {
        let num = reporter.total() + 1;
        match (test.run)() {
            Ok(()) => reporter.ok(num, test.name),
            Err(e) => reporter.not_ok(num, test.name, &e),
        }
    }
    eprintln!(
        "[* Suite {} done in {:.1}s]",
        suite.name,
        start.elapsed().as_secs_f64()
    );
}

fn now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() % 86400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let suite_name = args.get(1).map(|s| s.as_str()).unwrap_or("all");

    let selected: Vec<&Suite> = if suite_name == "all" {
        SUITES.iter().collect()
    } else {
        match SUITES.iter().find(|s| s.name == suite_name) {
            Some(s) => vec![s],
            None => {
                eprintln!("Unknown suite: {}", suite_name);
                eprintln!(
                    "Available: all, {}",
                    SUITES.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
                );
                process::exit(1);
            }
        }
    };

    let total_tests: usize = selected.iter().map(|s| s.tests.len()).sum();
    println!("1..{}", total_tests);

    let total_start = Instant::now();
    let mut reporter = TapReporter::new();
    for suite in &selected {
        run_suite(suite, &mut reporter);
    }

    eprintln!("");
    eprintln!(
        "{} passed, {} failed, {} skipped in {:.1}s",
        reporter.passed,
        reporter.failed,
        reporter.skipped,
        total_start.elapsed().as_secs_f64()
    );

    if !reporter.failures.is_empty() {
        eprintln!("Failed: {}", reporter.failures.join(", "));
        process::exit(1);
    }
}
