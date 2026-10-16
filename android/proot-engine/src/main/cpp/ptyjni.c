#include <jni.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <android/log.h>

#define UNUSED __attribute__((unused))
#define TAG "PTY"
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

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeForkPty(
    JNIEnv *env, jclass cls UNUSED, jstring jCmd, jobjectArray jArgs, jobjectArray jEnvVars,
    jint rows, jint cols) {

    int ptm_fd = open_ptm();
    if (ptm_fd < 0) return -1;

    if (setup_slave(ptm_fd) < 0) {
        close(ptm_fd);
        return -1;
    }

    struct winsize ws;
    memset(&ws, 0, sizeof(ws));
    ws.ws_row = (unsigned short) rows;
    ws.ws_col = (unsigned short) cols;
    ioctl(ptm_fd, TIOCSWINSZ, &ws);

    char *pts_slave = ptsname(ptm_fd);
    if (!pts_slave) {
        LOGE("ptsname: %s", strerror(errno));
        close(ptm_fd);
        return -1;
    }

    pid_t pid = fork();
    if (pid < 0) {
        LOGE("fork: %s", strerror(errno));
        close(ptm_fd);
        return -1;
    }

    if (pid == 0) {
        close(ptm_fd);
        setsid();

        int pts_fd = open(pts_slave, O_RDWR);
        if (pts_fd < 0) {
            LOGE("child: open pts: %s", strerror(errno));
            _exit(127);
        }

        dup2(pts_fd, STDIN_FILENO);
        dup2(pts_fd, STDOUT_FILENO);
        dup2(pts_fd, STDERR_FILENO);
        if (pts_fd > 2) close(pts_fd);

        if (jEnvVars) {
            jsize envCount = (*env)->GetArrayLength(env, jEnvVars);
            for (jsize i = 0; i + 1 < envCount; i += 2) {
                jstring jKey = (jstring)(*env)->GetObjectArrayElement(env, jEnvVars, i);
                jstring jVal = (jstring)(*env)->GetObjectArrayElement(env, jEnvVars, i + 1);
                const char *key = (*env)->GetStringUTFChars(env, jKey, NULL);
                const char *val = (*env)->GetStringUTFChars(env, jVal, NULL);
                setenv(key, val, 1);
                (*env)->ReleaseStringUTFChars(env, jKey, key);
                (*env)->ReleaseStringUTFChars(env, jVal, val);
            }
        }

        const char *cmd = (*env)->GetStringUTFChars(env, jCmd, NULL);

        if (jArgs) {
            jsize argc = (*env)->GetArrayLength(env, jArgs);
            char **argv = (char **) malloc(sizeof(char *) * (argc + 1));
            for (jsize i = 0; i < argc; i++) {
                jstring jArg = (jstring)(*env)->GetObjectArrayElement(env, jArgs, i);
                argv[i] = (char *) (*env)->GetStringUTFChars(env, jArg, NULL);
            }
            argv[argc] = NULL;
            execv(cmd, argv);
        } else {
            execl(cmd, cmd, NULL);
        }

        {
            char errbuf[256];
            int n = snprintf(errbuf, sizeof(errbuf),
                "execv failed for '%s': %s (errno=%d)\n", cmd, strerror(errno), errno);
            write(STDERR_FILENO, errbuf, n);
        }
        _exit(127);
    }

    last_child_pid = pid;
    return ptm_fd;
}

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeGetPid(JNIEnv *env UNUSED, jclass cls UNUSED) {
    return (jint) last_child_pid;
}

JNIEXPORT jint JNICALL
Java_id_or_oo_pr_engine_PtyNative_nativeRead(
    JNIEnv *env, jclass cls UNUSED, jint fd, jbyteArray jBuf, jint offset, jint length) {

    jbyte *buf = (*env)->GetByteArrayElements(env, jBuf, NULL);
    ssize_t n = read(fd, buf + offset, length);
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

    jbyte *buf = (*env)->GetByteArrayElements(env, jBuf, NULL);
    ssize_t n = write(fd, buf + offset, length);
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
    if (result == 0) return 0;
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    return -2;
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
