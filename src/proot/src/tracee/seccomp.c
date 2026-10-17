#include <stdio.h>     /* fprintf */
#include <errno.h>     /* E*, */
#include <signal.h>    /* SIGSYS, */
#include <unistd.h>    /* getpgid, */
#include <utime.h>     /* utimbuf, */
#include <sys/vfs.h>   /* statfs64 */
#include <sys/stat.h>  /* lstat, */
#include <string.h>    /* memset, strcpy */
#include <linux/net.h> /* SYS_SENDMMSG */
#include <assert.h>    /* assert(3), */
#include <time.h>      /* time(2), */
#include <talloc.h>    /* talloc_*, */
#include <fcntl.h>     /* AT_FDCWD, O_RDONLY, O_WRONLY, O_CREAT, O_TRUNC */
#include <sched.h>     /* CLONE_*, */
#include <limits.h>    /* PATH_MAX, */

#include "extension/extension.h"
#include "cli/note.h"
#include "syscall/chain.h"
#include "syscall/syscall.h"
#include "tracee/seccomp.h"
#include "tracee/mem.h"
#include "tracee/statx.h"
#include "path/path.h"

static int handle_seccomp_event_common(Tracee *tracee);

/**
 * Restart syscall that caused seccomp event
 * after changing it in tracee registers
 *
 * Syscall that will be restarted will be translated by proot
 * so SIGSYS handler sees untranslated paths and should leave
 * them untranslated.
 */
void restart_syscall_after_seccomp(Tracee* tracee) {
	word_t instr_pointer;

	/* Enable restore regs at end of replaced call.
	 * This also defers delivering of signals until restarted syscall finishes.  */
	tracee->restore_original_regs_after_seccomp_event = true;
	tracee->restart_how = PTRACE_SYSCALL;

	/* Move the instruction pointer back to the original trap */
	instr_pointer = peek_reg(tracee, CURRENT, INSTR_POINTER);
	poke_reg(tracee, INSTR_POINTER, instr_pointer - get_systrap_size(tracee));

	/* X86 usually uses orig_rax when selecting syscall,
	 * but as this code is happening outside syscall handler
	 * we need to copy orig_eax back to eax.  */
#if defined(ARCH_X86_64)
	tracee->_regs[CURRENT].rax = tracee->_regs[CURRENT].orig_rax;
#elif defined(ARCH_X86)
	tracee->_regs[CURRENT].eax = tracee->_regs[CURRENT].orig_eax;
#endif

	push_specific_regs(tracee, true);
}

/**
 * Set specified result (negative for errno) and do not restart syscall.
 */
void set_result_after_seccomp(Tracee *tracee, word_t result) {
	VERBOSE(tracee, 3, "Setting result after SIGSYS to 0x%lx", result);
	poke_reg(tracee, SYSARG_RESULT, result);
	push_specific_regs(tracee, false);
}

/**
 * Handle SIGSYS signal that was caused by system seccomp policy.
 *
 * Return 0 to swallow signal or SIGSYS to deliver it to process.
 */
int handle_seccomp_event(Tracee* tracee)
{
	int ret;

	/* Reset status so next SIGTRAP | 0x80 is
	 * recognized as syscall entry.  */
	tracee->status = 0;

	/* Registers are never restored at this stage as they weren't saved.  */
	tracee->restore_original_regs = false;

	/* Fetch registers.  */
	ret = fetch_regs(tracee);
	if (ret != 0) {
		VERBOSE(tracee, 1, "Couldn't fetch regs on seccomp SIGSYS");
		return SIGSYS;
	}

	/* Save regs so they can be restored at end of replaced call.  */
	save_current_regs(tracee, ORIGINAL_SECCOMP_REWRITE);

	/* X86 uses orig_rax when selecting syscall,
	 * however at this point we are after syscall has been rejected
	 * and orig_rax was reset to -1.  */
#if defined(ARCH_X86_64)
	tracee->_regs[CURRENT].orig_rax = tracee->_regs[CURRENT].rax;
#elif defined(ARCH_X86)
	tracee->_regs[CURRENT].orig_eax = tracee->_regs[CURRENT].eax;
#endif

	print_current_regs(tracee, 3, "seccomp SIGSYS");

	return handle_seccomp_event_common(tracee);
}

void fix_and_restart_enosys_syscall(Tracee* tracee)
{
	/* Reset tracee state so we're not handling syscall exit */
	tracee->status = 0;
	tracee->restore_original_regs = false;

	/* Restore and save original registers */
	memcpy(&tracee->_regs[CURRENT], &tracee->_regs[ORIGINAL], sizeof(tracee->_regs[CURRENT]));
	save_current_regs(tracee, ORIGINAL_SECCOMP_REWRITE);

	handle_seccomp_event_common(tracee);
}

static int handle_seccomp_event_common(Tracee *tracee)
{
	int ret;
	int status;
	Sysnum sysnum = get_sysnum(tracee, CURRENT);

	sysnum = get_sysnum(tracee, CURRENT);

	status = notify_extensions(tracee, SIGSYS_OCC, 0, 0);
	if (status < 0) {
		VERBOSE(tracee, 4, "SIGSYS errored out when being handled by an extension");
		set_result_after_seccomp(tracee, status);
		return 0;
	}
	if (status == 1) {
		VERBOSE(tracee, 4, "SIGSYS fully handled by an extension");
		set_result_after_seccomp(tracee, 0);
		return 0;
	}
	if (status == 2) {
		VERBOSE(tracee, 4, "SIGSYS fully handled by an extension with result set");
		return 0;
	}

	switch (sysnum) {
	case PR_open:
		set_sysnum(tracee, PR_openat);
		poke_reg(tracee, SYSARG_4, peek_reg(tracee, CURRENT, SYSARG_3));
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_accept:
		set_sysnum(tracee, PR_accept4);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_setgroups:
	case PR_setgroups32:
		set_result_after_seccomp(tracee, 0);
		break;

	case PR_getpgrp:
		/* Query value with getpgid and set it as result.  */
		set_result_after_seccomp(tracee, getpgid(tracee->pid));
		break;

	case PR_symlink:
		set_sysnum(tracee, PR_symlinkat);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_link:
		set_sysnum(tracee, PR_linkat);
		poke_reg(tracee, SYSARG_4, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		poke_reg(tracee, SYSARG_3, AT_FDCWD);
		poke_reg(tracee, SYSARG_5, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_chmod:
		set_sysnum(tracee, PR_fchmodat);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_fchmodat:
		set_result_after_seccomp(tracee, 0);
		break;

	case PR_chown:
	case PR_lchown:
	case PR_chown32:
	case PR_lchown32:
		set_sysnum(tracee, PR_fchownat);
		poke_reg(tracee, SYSARG_4, peek_reg(tracee, CURRENT, SYSARG_3));
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		if (sysnum == PR_lchown || sysnum == PR_lchown32) {
			poke_reg(tracee, SYSARG_5, AT_SYMLINK_NOFOLLOW);
		} else {
			poke_reg(tracee, SYSARG_5, 0);
		}
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_unlink:
	case PR_rmdir:
		set_sysnum(tracee, PR_unlinkat);
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		poke_reg(tracee, SYSARG_3, sysnum==PR_rmdir ? AT_REMOVEDIR : 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_send:
		set_sysnum(tracee, PR_sendto);
		poke_reg(tracee, SYSARG_5, 0);
		poke_reg(tracee, SYSARG_6, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_recv:
		set_sysnum(tracee, PR_recvfrom);
		poke_reg(tracee, SYSARG_5, 0);
		poke_reg(tracee, SYSARG_6, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_waitpid:
		set_sysnum(tracee, PR_wait4);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_statfs:
	{
		int size;
		int status;
		char path[PATH_MAX];
		char original[PATH_MAX];
		char devshm_path[PATH_MAX];
		struct statfs64 my_statfs64;
		struct compat_statfs my_statfs;
		size = read_string(tracee, original, peek_reg(tracee, CURRENT, SYSARG_1), PATH_MAX);
		if (size < 0) {
			set_result_after_seccomp(tracee, size);
			break;
		}
		if (size >= PATH_MAX) { 
			set_result_after_seccomp(tracee, -ENAMETOOLONG);
			break;
		}
            	translate_path(tracee, path, AT_FDCWD, original, true);
		status = statfs64(path, &my_statfs64);
		if (status < 0) {
			set_result_after_seccomp(tracee, -errno);
			break;
		}

		/* Fake /dev/shm being tmpfs, see statfs handler in syscall/exit.c */
		if (translate_path(tracee, devshm_path, AT_FDCWD, "/dev/shm", true) >= 0) {
			Comparison comparison = compare_paths(devshm_path, path);
			if (comparison == PATHS_ARE_EQUAL || comparison == PATH1_IS_PREFIX) {
				my_statfs64.f_type = 0x01021994;
			}
		}

		if ((my_statfs64.f_blocks | my_statfs64.f_bfree | my_statfs64.f_bavail |
     		     my_statfs64.f_bsize | my_statfs64.f_frsize | my_statfs64.f_files | 
		     my_statfs64.f_ffree) & 0xffffffff00000000ULL) { 
			set_result_after_seccomp(tracee, -EOVERFLOW);
			break;
		}
		my_statfs.f_type = my_statfs64.f_type;
		my_statfs.f_bsize = my_statfs64.f_bsize;
		my_statfs.f_blocks = my_statfs64.f_blocks;
		my_statfs.f_bfree = my_statfs64.f_bfree;
		my_statfs.f_bavail = my_statfs64.f_bavail;
		my_statfs.f_files = my_statfs64.f_files;
		my_statfs.f_ffree = my_statfs64.f_ffree;
		my_statfs.f_fsid = my_statfs64.f_fsid;
		my_statfs.f_namelen = my_statfs64.f_namelen;
		my_statfs.f_frsize = my_statfs64.f_frsize;
		my_statfs.f_flags = my_statfs64.f_flags;
		memset(my_statfs.f_spare, 0, sizeof(my_statfs.f_spare));
                write_data(tracee, peek_reg(tracee, CURRENT, SYSARG_2), &my_statfs, sizeof(struct compat_statfs));

		set_result_after_seccomp(tracee, 0);
		break;
	}

	case PR_utimes:
	{
		/* int utimes(const char *filename, const struct timeval times[2]);
		 *
		 * convert to:
		 * int utimensat(int dirfd, const char *pathname, const struct timespec times[2], int flags);  */
		struct timeval times[2];
		struct timespec timens[2];

		set_sysnum(tracee, PR_utimensat);
		if (peek_reg(tracee, CURRENT, SYSARG_2) != 0) {
			ret = read_data(tracee, times, peek_reg(tracee, CURRENT, SYSARG_2), sizeof(times));
			if (ret < 0) {
				set_result_after_seccomp(tracee, ret);
				break;
			}
			timens[0].tv_sec = (time_t)times[0].tv_sec;
			timens[0].tv_nsec = (long)times[0].tv_usec * 1000;
			timens[1].tv_sec = (time_t)times[1].tv_sec;
			timens[1].tv_nsec = (long)times[1].tv_usec * 1000;
			ret = set_sysarg_data(tracee, timens, sizeof(timens), SYSARG_2);
			if (ret < 0) {
				set_result_after_seccomp(tracee, ret);
				break;
			}
		}
		poke_reg(tracee, SYSARG_4, 0);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_utime:
	{
		/* int utime(const char *filename, const struct utimbuf *times);
		 *
		 * convert to:
		 * int utimensat(int dirfd, const char *pathname, const struct timespec times[2], int flags);  */
		struct utimbuf times;
		struct timespec timens[2];

		set_sysnum(tracee, PR_utimensat);
		if (peek_reg(tracee, CURRENT, SYSARG_2) != 0) {
			ret = read_data(tracee, &times, peek_reg(tracee, CURRENT, SYSARG_2), sizeof(times));
			if (ret < 0) {
				set_result_after_seccomp(tracee, ret);
				break;
			}
			timens[0].tv_sec = (time_t)times.actime;
			timens[0].tv_nsec = 0;
			timens[1].tv_sec = (time_t)times.modtime;
			timens[1].tv_nsec = 0;
			ret = set_sysarg_data(tracee, timens, sizeof(timens), SYSARG_2);
			if (ret < 0) {
				set_result_after_seccomp(tracee, ret);
				break;
			}
		}
		poke_reg(tracee, SYSARG_4, 0);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;
	}

#if defined(ARCH_X86) || defined(ARCH_X86_64)
	case PR_sendmmsg:
	{
		/* Convert direct sendmmsg syscall to socketcall.
		 * This affects only 32-bit x86, in other archs
		 * bionic doesn't use socketcall() for sendmmsg.  */
		size_t arg_size = sizeof_word(tracee);
		assert(arg_size <= sizeof(word_t));
		byte_t args[arg_size * 4];
		memset(args, 0, arg_size * 4);
		*(word_t*)(args) = peek_reg(tracee, CURRENT, SYSARG_1);
		*(word_t*)(args + arg_size) = peek_reg(tracee, CURRENT, SYSARG_2);
		*(word_t*)(args + 2 * arg_size) = peek_reg(tracee, CURRENT, SYSARG_3);
		*(word_t*)(args + 3 * arg_size) = peek_reg(tracee, CURRENT, SYSARG_4);
		word_t tracee_args = alloc_mem(tracee, arg_size * 4);
		write_data(tracee, tracee_args, args, arg_size * 4);
		set_sysnum(tracee, PR_socketcall);
		poke_reg(tracee, SYSARG_1, SYS_SENDMMSG);
		poke_reg(tracee, SYSARG_2, tracee_args);
		restart_syscall_after_seccomp(tracee);
		break;
	}
#endif

	case PR_stat:
	case PR_lstat:
		set_sysnum(tracee, PR_newfstatat);
		poke_reg(tracee, SYSARG_4, sysnum == PR_lstat ? AT_SYMLINK_NOFOLLOW : 0);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_pipe:
		set_sysnum(tracee, PR_pipe2);
		poke_reg(tracee, SYSARG_2, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_dup2:
		set_sysnum(tracee, PR_dup3);
		poke_reg(tracee, SYSARG_3, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_access:
		set_sysnum(tracee, PR_faccessat);
		poke_reg(tracee, SYSARG_4, 0);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_mkdir:
		set_sysnum(tracee, PR_mkdirat);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_rename:
		set_sysnum(tracee, PR_renameat);
		poke_reg(tracee, SYSARG_4, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_3, AT_FDCWD);
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_select:
	{
		// TODO: This doesn't update timeout with time spent inside select(2)
		//       after returning from syscall
		word_t timeval_arg = peek_reg(tracee, CURRENT, SYSARG_5);
		word_t timespec_arg = 0;
		if (timeval_arg != 0) {
			struct timeval tv = {};
			if (read_data(tracee, &tv, timeval_arg, sizeof(tv))) {
				set_result_after_seccomp(tracee, -EFAULT);
				break;
			}
			if (tv.tv_usec >= 1000000 || tv.tv_usec < 0) {
				set_result_after_seccomp(tracee, -EINVAL);
				break;
			}
			struct timespec ts = {
				.tv_sec = tv.tv_sec,
				.tv_nsec = tv.tv_usec * 1000
			};
			timespec_arg = alloc_mem(tracee, sizeof(ts));
			if(write_data(tracee, timespec_arg, &ts, sizeof(ts))) {
				set_result_after_seccomp(tracee, -EFAULT);
				break;
			}
		}
		set_sysnum(tracee, PR_pselect6);
		poke_reg(tracee, SYSARG_5, timespec_arg);
		poke_reg(tracee, SYSARG_6, 0);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_poll:
	{
		int ms_arg = (int) peek_reg(tracee, CURRENT, SYSARG_3);
		word_t timespec_arg = 0;
		if (ms_arg >= 0) {
			struct timespec ts = {
				.tv_sec = ms_arg / 1000,
				.tv_nsec = (ms_arg % 1000) * 1000000
			};
			timespec_arg = alloc_mem(tracee, sizeof(ts));
			if(write_data(tracee, timespec_arg, &ts, sizeof(ts))) {
				set_result_after_seccomp(tracee, -EFAULT);
				break;
			}
		}
		set_sysnum(tracee, PR_ppoll);
		poke_reg(tracee, SYSARG_3, timespec_arg);
		poke_reg(tracee, SYSARG_4, 0);
		poke_reg(tracee, SYSARG_5, 0);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_epoll_wait:
	{
		set_sysnum(tracee, PR_epoll_pwait);
		poke_reg(tracee, SYSARG_5, 0);
		poke_reg(tracee, SYSARG_6, 0);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_time:
	{
		time_t t = time(NULL);
		word_t addr = peek_reg(tracee, CURRENT, SYSARG_1);
		errno = 0;
		if (addr != 0) {
			poke_word(tracee, addr, t);
		}
		set_result_after_seccomp(tracee, errno ? -EFAULT : t);
		break;
	}

	case PR_statx:
	{
		set_result_after_seccomp(tracee, handle_statx_syscall(tracee, true));
		break;
	}

	case PR_ftruncate:
	{
		if (detranslate_sysnum(get_abi(tracee), PR_ftruncate64) == SYSCALL_AVOIDER) {
			set_result_after_seccomp(tracee, -ENOSYS);
			break;
		}
		set_sysnum(tracee, PR_ftruncate64);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, 0);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_setresuid:
	case PR_setresgid:
	{
		gid_t rxid, exid, sxid, rxid_, exid_, sxid_;
		rxid = peek_reg(tracee, CURRENT, SYSARG_1);
		exid = peek_reg(tracee, CURRENT, SYSARG_2);
		sxid = peek_reg(tracee, CURRENT, SYSARG_3);
		if (sysnum == PR_setresuid)
			ret = getresuid(&rxid_, &exid_, &sxid_);
		else
			ret = getresgid(&rxid_, &exid_, &sxid_);
		if (ret) {  // EFAULT = address outside address space
			set_result_after_seccomp(tracee, -EPERM);
			break;
		}
		ret = 0;
		if (rxid != rxid_ && rxid != (gid_t) -1)
			ret = -EPERM;
		if (exid != exid_ && exid != (gid_t) -1)
			ret = -EPERM;
		if (sxid != sxid_ && sxid != (gid_t) -1)
			ret = -EPERM;
		set_result_after_seccomp(tracee, ret);
		break;
	}

	case PR_chdir:
	{
		char path[PATH_MAX];
		char translated[PATH_MAX];
		int size;

		size = read_string(tracee, path, peek_reg(tracee, ORIGINAL, SYSARG_1), PATH_MAX);
		if (size < 0) {
			set_result_after_seccomp(tracee, size);
			break;
		}
		if (size >= PATH_MAX) {
			set_result_after_seccomp(tracee, -ENAMETOOLONG);
			break;
		}

		status = translate_path(tracee, translated, AT_FDCWD, path, true);
		if (status < 0) {
			set_result_after_seccomp(tracee, status);
			break;
		}

		set_result_after_seccomp(tracee, 0);

		status = detranslate_path(tracee, translated, NULL);
		if (status >= 0) {
			chop_finality(translated);
			{
				char *tmp = talloc_strdup(tracee->fs, translated);
				if (tmp != NULL) {
					TALLOC_FREE(tracee->fs->cwd);
					tracee->fs->cwd = tmp;
					talloc_set_name_const(tracee->fs->cwd, "$cwd");
				}
			}
		}
		break;
	}

	case PR_fchdir:
	{
		char path[PATH_MAX];
		char translated[PATH_MAX];
		int dirfd = peek_reg(tracee, ORIGINAL, SYSARG_1);
		int status2;

		strcpy(path, ".");
		status2 = translate_path(tracee, translated, dirfd, path, true);
		if (status2 < 0) {
			set_result_after_seccomp(tracee, status2);
			break;
		}

		set_result_after_seccomp(tracee, 0);

		status2 = detranslate_path(tracee, translated, NULL);
		if (status2 >= 0) {
			chop_finality(translated);
			{
				char *tmp = talloc_strdup(tracee->fs, translated);
				if (tmp != NULL) {
					TALLOC_FREE(tracee->fs->cwd);
					tracee->fs->cwd = tmp;
					talloc_set_name_const(tracee->fs->cwd, "$cwd");
				}
			}
		}
		break;
	}

	case PR_linkat:
		set_result_after_seccomp(tracee, -EPERM);
		break;

	case PR_getcwd:
	{
		size_t size = (size_t) peek_reg(tracee, ORIGINAL, SYSARG_2);
		if (size == 0) {
			set_result_after_seccomp(tracee, -EINVAL);
			break;
		}

		char path[PATH_MAX];
		int status2 = translate_path(tracee, path, AT_FDCWD, ".", false);
		if (status2 < 0) {
			set_result_after_seccomp(tracee, status2);
			break;
		}

		size_t new_size = strlen(tracee->fs->cwd) + 1;
		if (size < new_size) {
			set_result_after_seccomp(tracee, -ERANGE);
			break;
		}

		word_t output = peek_reg(tracee, ORIGINAL, SYSARG_1);
		status2 = write_data(tracee, output, tracee->fs->cwd, new_size);
		if (status2 < 0) {
			set_result_after_seccomp(tracee, status2);
			break;
		}

		set_result_after_seccomp(tracee, new_size);
		break;
	}

	case PR_set_robust_list:
		set_result_after_seccomp(tracee, -ENOSYS);
		break;

	case PR_faccessat:
		set_result_after_seccomp(tracee, 0);
		break;

	case PR_faccessat2:
		set_sysnum(tracee, PR_faccessat);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_openat2:
		set_sysnum(tracee, PR_openat);
		poke_reg(tracee, SYSARG_5, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_renameat2:
		set_sysnum(tracee, PR_renameat);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_process_madvise:
		set_result_after_seccomp(tracee, 0);
		break;

	case PR_clone3:
	{
		word_t args_ptr = peek_reg(tracee, ORIGINAL, SYSARG_1);
		word_t flags = peek_word(tracee, args_ptr);
		if (flags & CLONE_THREAD) {
			set_result_after_seccomp(tracee, -ENOSYS);
			break;
		}
		flags &= ~(word_t)(CLONE_VM | CLONE_VFORK);
		word_t child_tid = peek_word(tracee, args_ptr + 16);
		word_t parent_tid = peek_word(tracee, args_ptr + 24);
		word_t exit_signal = peek_word(tracee, args_ptr + 32);
		word_t stack = peek_word(tracee, args_ptr + 40);
		word_t tls = peek_word(tracee, args_ptr + 56);
		set_sysnum(tracee, PR_clone);
		poke_reg(tracee, SYSARG_1, flags | exit_signal);
		poke_reg(tracee, SYSARG_2, stack);
		poke_reg(tracee, SYSARG_3, parent_tid);
		poke_reg(tracee, SYSARG_4, child_tid);
		poke_reg(tracee, SYSARG_5, tls);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_clone:
	{
		word_t flags = peek_reg(tracee, ORIGINAL, SYSARG_1);
		if (flags & CLONE_THREAD) {
			set_result_after_seccomp(tracee, -ENOSYS);
			break;
		}
		flags &= ~(word_t)(CLONE_VM | CLONE_VFORK);
		poke_reg(tracee, SYSARG_1, flags);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_setuid:
	case PR_setgid:
	case PR_setreuid:
	case PR_setregid:
	case PR_setfsuid:
	case PR_setfsgid:
		set_result_after_seccomp(tracee, 0);
		break;

	default:
	{
		word_t kernel_num = peek_reg(tracee, CURRENT, SYSARG_NUM);
		const char *log_path = "/data/data/id.or.oo.pr/cache/sigsys-log.txt";
		FILE *f = fopen(log_path, "a");
		if (f) {
			time_t now = time(NULL);
			struct tm *tm = localtime(&now);
			char comm[64];
			char timebuf[32];
			FILE *commf;
			strftime(timebuf, sizeof(timebuf), "%H:%M:%S", tm);
			memset(comm, 0, sizeof(comm));
			commf = fopen("/proc/self/comm", "r");
			if (commf) {
				if (fgets(comm, sizeof(comm), commf)) {
					char *nl = strchr(comm, '\n');
					if (nl) *nl = '\0';
				}
				fclose(commf);
			}
			fprintf(f, "SIGSYS: time=%s pid=%d comm=%s kernel_num=%lu pr=%d\n",
				timebuf, (int)tracee->pid, comm,
				(unsigned long)kernel_num, (int)sysnum);
			fclose(f);
		}
		if (sysnum == PR_openat || sysnum == PR_fstatat64) {
			set_result_after_seccomp(tracee, -ENOENT);
		} else {
			set_result_after_seccomp(tracee, -ENOSYS);
		}
		break;
	}
	}

	return 0;
}
