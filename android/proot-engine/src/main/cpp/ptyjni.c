#include <jni.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <android/log.h>

#define UNUSED __attribute__((unused))
#define TAG "PTY"
#define STILL_RUNNING (-2)
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, TAG, __VA_ARGS__)

static int open_ptm(void) {
    int fd = open("/dev/ptmx", O_RDWR | O_NOCTTY);
    if (fd < 0) {
        LOGE("open /dev/ptmx: %s", strerror(errno));
        return -1;
    }
    return fd;
}

static int setup_slave(int ptm_fd) {
    if (grantpt(ptm_fd) < 0) {
        LOGE("grantpt: %s", strerror(errno));
        return -1;
    }
    if (unlockpt(ptm_fd) < 0) {
        LOGE("unlockpt: %s", strerror(errno));
        return -1;
    }
    return 0;
}

static pid_t last_child_pid = -1;

extern char **environ;

static void free_strings(char **strings) {
    if (!strings) return;
    for (char **p = strings; *p; p++) free(*p);
    free(strings);
}

static char *dup_jstring(JNIEnv *env, jstring js) {
    if (!js) return NULL;
    const char *utf = (*env)->GetStringUTFChars(env, js, NULL);
    if (!utf) return NULL;
    char *copy = strdup(utf);
    (*env)->ReleaseStringUTFChars(env, js, utf);
    return copy;
}

static char *dup_array_element(JNIEnv *env, jobjectArray array, jsize i) {
    jstring js = (jstring) (*env)->GetObjectArrayElement(env, array, i);
    char *copy = dup_jstring(env, js);
    (*env)->DeleteLocalRef(env, js);
    return copy;
}

static char **build_argv(JNIEnv *env, const char *cmd, jobjectArray jArgs) {
    jsize argc = jArgs ? (*env)->GetArrayLength(env, jArgs) : 0;
    char **argv = calloc((size_t) (argc > 0 ? argc : 1) + 1, sizeof(char *));
    if (!argv) return NULL;
    if (argc == 0) {
        argv[0] = strdup(cmd);
        if (!argv[0]) {
            free(argv);
            return NULL;
        }
        return argv;
    }
    for (jsize i = 0; i < argc; i++) {
        argv[i] = dup_array_element(env, jArgs, i);
        if (!argv[i]) {
            free_strings(argv);
            return NULL;
        }
    }
    return argv;
}

static size_t name_length(const char *entry) {
    const char *eq = strchr(entry, '=');
    return eq ? (size_t) (eq - entry) : strlen(entry);
}

static bool same_name(const char *a, const char *b) {
    size_t n = name_length(a);
    return n == name_length(b) && strncmp(a, b, n) == 0;
}

static bool overridden(char **overrides, size_t from, const char *entry) {
    for (size_t i = from; overrides[i]; i++)
        if (same_name(overrides[i], entry)) return true;
    return false;
}

static char **build_envp(JNIEnv *env, jobjectArray jEnvVars) {
    jsize pairs = jEnvVars ? (*env)->GetArrayLength(env, jEnvVars) / 2 : 0;
    char **overrides = calloc((size_t) pairs + 1, sizeof(char *));
    if (!overrides) return NULL;
    for (jsize i = 0; i < pairs; i++) {
        char *key = dup_array_element(env, jEnvVars, 2 * i);
        char *val = dup_array_element(env, jEnvVars, 2 * i + 1);
        if (!key || !val || asprintf(&overrides[i], "%s=%s", key, val) < 0) {
            overrides[i] = NULL;
            free(key);
            free(val);
            free_strings(overrides);
            return NULL;
        }
        free(key);
        free(val);
    }

    size_t inherited = 0;
    while (environ && environ[inherited]) inherited++;
    char **envp = calloc(inherited + (size_t) pairs + 1, sizeof(char *));
    if (!envp) {
        free_strings(overrides);
        return NULL;
    }
    size_t n = 0;
    for (size_t i = 0; i < inherited; i++) {
        if (overridden(overrides, 0, environ[i])) continue;
        envp[n] = strdup(environ[i]);
        if (!envp[n]) goto fail;
        n++;
    }
    for (jsize i = 0; i < pairs; i++) {
        if (overridden(overrides, (size_t) i + 1, overrides[i])) continue;
        envp[n] = strdup(overrides[i]);
        if (!envp[n]) goto fail;
        n++;
    }
    free_strings(overrides);
    return envp;

fail:
    free_strings(overrides);
    free_strings(envp);
    return NULL;
}

static void write_str(const char *s) {
    (void) write(STDERR_FILENO, s, strlen(s));
}

static void reset_signals(void) {
    sigset_t none;
    sigemptyset(&none);
    sigprocmask(SIG_SETMASK, &none, NULL);
    for (int sig = 1; sig < NSIG; sig++)
        if (sig != SIGKILL && sig != SIGSTOP) signal(sig, SIG_DFL);
}

static void close_inherited_fds(void) {
    if (syscall(SYS_close_range, 3, ~0U, 0) == 0) return;
    for (int fd = 3; fd < 65536; fd++) close(fd);
}

static void exec_child(const char *pts_name, const char *cmd, char **argv, char **envp) {
    reset_signals();
    setsid();
    int pts_fd = open(pts_name, O_RDWR);
    if (pts_fd < 0) _exit(127);
    dup2(pts_fd, STDIN_FILENO);
    dup2(pts_fd, STDOUT_FILENO);
    dup2(pts_fd, STDERR_FILENO);
    close_inherited_fds();

    execve(cmd, argv, envp);

    int err = errno;
    char digits[12];
    int i = sizeof(digits);
    digits[--i] = '\0';
    do {
        digits[--i] = (char) ('0' + err % 10);
        err /= 10;
    } while (err > 0 && i > 0);
    write_str("execve failed for '");
    write_str(cmd);
    write_str("' (errno ");
    write_str(&digits[i]);
    write_str(")\n");
    _exit(127);
}

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeForkPty(
    JNIEnv *env, jclass cls UNUSED, jstring jCmd, jobjectArray jArgs, jobjectArray jEnvVars,
    jint rows, jint cols) {

    char *cmd = dup_jstring(env, jCmd);
    char **argv = cmd ? build_argv(env, cmd, jArgs) : NULL;
    char **envp = argv ? build_envp(env, jEnvVars) : NULL;
    if (!envp) {
        LOGE("forkPty: could not copy the command, arguments or environment");
        goto fail;
    }

    int ptm_fd = open_ptm();
    if (ptm_fd < 0) goto fail;

    if (setup_slave(ptm_fd) < 0) {
        close(ptm_fd);
        goto fail;
    }

    struct winsize ws;
    memset(&ws, 0, sizeof(ws));
    ws.ws_row = (unsigned short) rows;
    ws.ws_col = (unsigned short) cols;
    ioctl(ptm_fd, TIOCSWINSZ, &ws);

    char pts_name[64];
    int err = ptsname_r(ptm_fd, pts_name, sizeof(pts_name));
    if (err != 0) {
        LOGE("ptsname_r: %s", strerror(err));
        close(ptm_fd);
        goto fail;
    }

    pid_t pid = fork();
    if (pid < 0) {
        LOGE("fork: %s", strerror(errno));
        close(ptm_fd);
        goto fail;
    }

    if (pid == 0) {
        close(ptm_fd);
        exec_child(pts_name, cmd, argv, envp);
    }

    free(cmd);
    free_strings(argv);
    free_strings(envp);
    last_child_pid = pid;
    return ptm_fd;

fail:
    free(cmd);
    free_strings(argv);
    free_strings(envp);
    return -1;
}

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeGetPid(JNIEnv *env UNUSED, jclass cls UNUSED) {
    return (jint) last_child_pid;
}

static bool slice_in_bounds(JNIEnv *env, jbyteArray jBuf, jint offset, jint length) {
    jsize size = (*env)->GetArrayLength(env, jBuf);
    if (offset < 0 || length < 0 || offset > size - length) {
        jclass cls = (*env)->FindClass(env, "java/lang/ArrayIndexOutOfBoundsException");
        if (cls) (*env)->ThrowNew(env, cls, "offset/length outside the array");
        return false;
    }
    return true;
}

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeRead(
    JNIEnv *env, jclass cls UNUSED, jint fd, jbyteArray jBuf, jint offset, jint length) {

    if (!slice_in_bounds(env, jBuf, offset, length)) return -1;
    jbyte *buf = (*env)->GetByteArrayElements(env, jBuf, NULL);
    if (!buf) return -1;
    ssize_t n = read(fd, buf + offset, (size_t) length);
    (*env)->ReleaseByteArrayElements(env, jBuf, buf, 0);

    if (n < 0) {
        if (errno == EAGAIN || errno == EINTR) return 0;
        return -1;
    }
    return (jint) n;
}

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeWrite(
    JNIEnv *env, jclass cls UNUSED, jint fd, jbyteArray jBuf, jint offset, jint length) {

    if (!slice_in_bounds(env, jBuf, offset, length)) return -1;
    jbyte *buf = (*env)->GetByteArrayElements(env, jBuf, NULL);
    if (!buf) return -1;
    ssize_t n = write(fd, buf + offset, (size_t) length);
    (*env)->ReleaseByteArrayElements(env, jBuf, buf, JNI_ABORT);

    if (n < 0) {
        if (errno == EAGAIN || errno == EINTR) return 0;
        return -1;
    }
    return (jint) n;
}

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeResize(
    JNIEnv *env UNUSED, jclass cls UNUSED, jint fd, jint rows, jint cols) {

    struct winsize ws;
    memset(&ws, 0, sizeof(ws));
    ws.ws_row = (unsigned short) rows;
    ws.ws_col = (unsigned short) cols;
    return ioctl(fd, TIOCSWINSZ, &ws);
}

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeWaitPid(JNIEnv *env UNUSED, jclass cls UNUSED, jint pid) {
    int status;
    pid_t result = waitpid(pid, &status, WNOHANG);
    if (result < 0) return -1;
    if (result == 0) return STILL_RUNNING;
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return -1;
}

/* Block until @pid exits and reap it: exit code, or 128 + signal; -1 on error. */
JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeWaitPidBlocking(JNIEnv *env UNUSED, jclass cls UNUSED, jint pid) {
    int status;
    pid_t result;
    do {
        result = waitpid(pid, &status, 0);
    } while (result < 0 && errno == EINTR);
    if (result < 0) return -1;
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return -1;
}

JNIEXPORT void JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeClose(JNIEnv *env UNUSED, jclass cls UNUSED, jint fd) {
    close(fd);
}
