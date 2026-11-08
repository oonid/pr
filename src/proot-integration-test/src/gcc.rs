use super::TestResult;

pub fn probe() -> bool {
    std::path::Path::new("/usr/bin/cc").exists() || std::path::Path::new("/usr/bin/gcc").exists()
}

fn run_sh(cmd: &str) -> Result<std::process::Output, String> {
    std::process::Command::new("/bin/sh")
        .args(["-c", cmd])
        .output()
        .map_err(|e| format!("sh: {}", e))
}

pub fn test_search_dirs() -> TestResult {
    let out = run_sh("cc -print-search-dirs 2>&1")?;
    let stdout = std::str::from_utf8(&out.stdout).unwrap_or("");
    if stdout.contains("install:") && !stdout.contains(".l2s") {
        Ok(())
    } else {
        Err(format!("cc -print-search-dirs: {:?}", stdout))
    }
}

pub fn test_compile_c() -> TestResult {
    let src = "#include <stdio.h>\nint main(){printf(\"ok\\n\");return 0;}";
    std::fs::write("/tmp/pit-test.c", src).map_err(|e| format!("write: {}", e))?;
    let out = run_sh("cc /tmp/pit-test.c -o /tmp/pit-test 2>&1")?;
    if !out.status.success() {
        let _ = std::fs::remove_file("/tmp/pit-test.c");
        return Err(format!(
            "cc failed: {:?}",
            std::str::from_utf8(&out.stdout).unwrap_or("")
        ));
    }
    let out2 = run_sh("/tmp/pit-test 2>&1")?;
    let _ = std::fs::remove_file("/tmp/pit-test");
    let _ = std::fs::remove_file("/tmp/pit-test.c");
    let stdout = std::str::from_utf8(&out2.stdout).unwrap_or("").trim();
    if stdout == "ok" {
        Ok(())
    } else {
        Err(format!("expected 'ok', got {:?}", stdout))
    }
}

pub fn test_proc_exe() -> TestResult {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {}", e))?;
    if exe.to_str().unwrap_or("").contains(".l2s") {
        Err(format!("exe contains .l2s: {:?}", exe))
    } else {
        Ok(())
    }
}

fn compile_and_run(name: &str, src: &str) -> Result<String, String> {
    let c = format!("/tmp/pit-{}.c", name);
    let bin = format!("/tmp/pit-{}", name);
    std::fs::write(&c, src).map_err(|e| format!("write: {}", e))?;
    let out = run_sh(&format!("cc {} -o {} 2>&1", c, bin))?;
    let _ = std::fs::remove_file(&c);
    if !out.status.success() {
        return Err(format!(
            "cc failed: {:?}",
            std::str::from_utf8(&out.stdout).unwrap_or("")
        ));
    }
    let run = run_sh(&format!("timeout 20 {} 2>&1; echo exit=$?", bin))?;
    let _ = std::fs::remove_file(&bin);
    Ok(std::str::from_utf8(&run.stdout)
        .unwrap_or("")
        .trim()
        .to_string())
}

pub fn test_posix_spawn_missing_program() -> TestResult {
    let src = "#include <spawn.h>\n#include <stdio.h>\n#include <sys/wait.h>\n\
               extern char **environ;\n\
               int main(void){pid_t p;int st;char *a[]={\"x\",0};\
               int rc=posix_spawn(&p,\"/nonexistent-pit-prog\",0,0,a,environ);\
               if(rc==0)waitpid(p,&st,0);printf(\"rc=%d\\n\",rc);return 0;}";
    let out = compile_and_run("spawn", src)?;
    if out.contains("rc=2") && out.contains("exit=0") {
        Ok(())
    } else {
        Err(format!(
            "posix_spawn of a missing program: {:?}, want rc=2 (ENOENT)",
            out
        ))
    }
}

pub fn test_vfork_exec_returns() -> TestResult {
    let src = "#include <stdio.h>\n#include <unistd.h>\n#include <sys/wait.h>\n\
               int main(void){int st;pid_t p=vfork();\
               if(p==0){char *a[]={\"true\",0};execvp(\"true\",a);_exit(127);}\
               waitpid(p,&st,0);int one=WEXITSTATUS(st);\
               p=vfork();if(p==0){char *a[]={\"x\",0};execv(\"/nonexistent-pit-prog\",a);_exit(126);}\
               waitpid(p,&st,0);printf(\"true=%d missing=%d\\n\",one,WEXITSTATUS(st));return 0;}";
    let out = compile_and_run("vfork", src)?;
    if out.contains("true=0 missing=126") && out.contains("exit=0") {
        Ok(())
    } else {
        Err(format!(
            "vfork and exec: {:?}, want true=0 missing=126",
            out
        ))
    }
}
