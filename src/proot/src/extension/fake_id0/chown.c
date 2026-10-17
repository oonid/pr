#include <unistd.h>      /* get*id(2),  */
#include <linux/limits.h>
#include <errno.h>
#include <fcntl.h>       /* AT_*,  */
#include <stdio.h>       /* snprintf,  */
#include <sys/stat.h>    /* fstatat,  */

#include "syscall/sysnum.h"
#include "tracee/mem.h"
#include "extension/fake_id0/chown.h"
#include "extension/fake_id0/helper_functions.h"

#ifndef USERLAND
static bool fd_exists(const Tracee *tracee, int fd)
{
	char link[64];
	struct stat statl;

	if (fd < 0 && fd != AT_FDCWD)
		return false;
	if (fd == AT_FDCWD)
		snprintf(link, sizeof(link), "/proc/%d/cwd", tracee->pid);
	else
		snprintf(link, sizeof(link), "/proc/%d/fd/%d", tracee->pid, fd);
	return stat(link, &statl) == 0;
}

static bool chown_target_exists(const Tracee *tracee)
{
	char path[PATH_MAX];
	struct stat statl;
	Reg path_sysarg = SYSARG_1;
	int flags = 0;

	switch (get_sysnum(tracee, ORIGINAL)) {
	case PR_fchown:
	case PR_fchown32:
		return (int) peek_reg(tracee, CURRENT, SYSARG_1) >= 0
			&& fd_exists(tracee, (int) peek_reg(tracee, CURRENT, SYSARG_1));

	case PR_lchown:
	case PR_lchown32:
		flags = AT_SYMLINK_NOFOLLOW;
		break;

	case PR_fchownat:
		path_sysarg = SYSARG_2;
		flags = (int) peek_reg(tracee, CURRENT, SYSARG_5) & AT_SYMLINK_NOFOLLOW;
		break;

	default:
		break;
	}

	if (read_path(tracee, path, peek_reg(tracee, CURRENT, path_sysarg)) < 0)
		return false;
	if (path[0] == '\0')
		return get_sysnum(tracee, ORIGINAL) == PR_fchownat
			&& (peek_reg(tracee, CURRENT, SYSARG_5) & AT_EMPTY_PATH) != 0
			&& fd_exists(tracee, (int) peek_reg(tracee, CURRENT, SYSARG_1));
	return fstatat(AT_FDCWD, path, &statl, flags) == 0;
}

int handle_chown_enter_end(Tracee *tracee, Config *config, Reg uid_sysarg, Reg gid_sysarg) {
	uid_t uid;
	gid_t gid;

	uid = peek_reg(tracee, ORIGINAL, uid_sysarg);
	gid = peek_reg(tracee, ORIGINAL, gid_sysarg);

	/* Swap actual and emulated ids to get a chance of
	 * success.  */
	if (uid == config->ruid)
		poke_reg(tracee, uid_sysarg, getuid());
	if (gid == config->rgid)
		poke_reg(tracee, gid_sysarg, getgid());

	/* On Android the kernel (SELinux untrusted_app domain) returns ENOENT
	 * (a masqueraded EPERM) for chown/lchown on files inside the app data
	 * dir — even when uid/gid already match the app's own ids.  dpkg treats
	 * EPERM as ignorable but aborts on ENOENT, so the install fails.
	 *
	 * Fix: replace the syscall with getuid, which fake_id0 will intercept
	 * in the EXIT phase and fake to return config->ruid (0).  The caller
	 * therefore sees lchown returning 0 (success) and the kernel never
	 * attempts the operation at all.  A missing target or a bad fd keeps
	 * the real syscall, so the caller gets the real error. */
	if (chown_target_exists(tracee))
		set_sysnum(tracee, PR_getuid);

	return 0;
}
#endif /* ifndef USERLAND */

#ifdef USERLAND
/** Handles chown, lchown, fchown, and fchownat syscalls. Changes the meta file
 *  to reflect arguments sent to the syscall if the meta file exists. See
 *  chown(2) for returned permission errors.
 */
int handle_chown_enter_end(Tracee *tracee, Reg path_sysarg, Reg owner_sysarg,
	Reg group_sysarg, Reg fd_sysarg, Reg dirfd_sysarg, Config *config)
{
	int status;
	mode_t mode;
	uid_t owner, read_owner;
	gid_t group, read_group;
	char path[PATH_MAX];
	char rel_path[PATH_MAX];
	char meta_path[PATH_MAX];
	
	if(path_sysarg == IGNORE_SYSARG)
		status = get_fd_path(tracee, path, fd_sysarg, CURRENT);
	else
		status = read_sysarg_path(tracee, path, path_sysarg, CURRENT);
	if(status < 0)
		return status;
	// If the path exists outside the guestfs, drop the syscall.
	else if(status == 1) {
		set_sysnum(tracee, PR_getuid);
		return 0;
	}

	status = get_meta_path(path, meta_path);
	if(status < 0)
		return status;

	if(path_exists(meta_path) != 0)
		return 0;

	status = get_fd_path(tracee, rel_path, dirfd_sysarg, CURRENT);
	if(status < 0)
		return status;

	status = check_dir_perms(tracee, 'r', path, rel_path, config);
	if(status < 0)
		return status;

	read_meta_file(meta_path, &mode, &read_owner, &read_group, config);
	owner = peek_reg(tracee, ORIGINAL, owner_sysarg);
	/** When chown is called without an owner specified, eg 
	 *  chown :1000 'file', the owner argument to the system call is implicitly
	 *  set to -1. To avoid this, the owner argument is replaced with the owner
	 *  according to the meta file if it exists, or the current euid.
	 */
	if((int) owner == -1)
		owner = read_owner;
	group = peek_reg(tracee, ORIGINAL, group_sysarg);
	if(config->euid == 0) 
		write_meta_file(meta_path, mode, owner, group, 0, config);

	//TODO Handle chown properly: owner can only change the group of
	//  a file to another group they belong to.
	else if(config->euid == read_owner) {
		write_meta_file(meta_path, mode, read_owner, group, 0, config);
		poke_reg(tracee, owner_sysarg, read_owner);	
	}

	else if(config->euid != read_owner) 
		return -EPERM;

	set_sysnum(tracee, PR_getuid);

	return 0;
}
#endif /* ifdef USERLAND */
