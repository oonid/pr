#include <stdio.h>     /* rename(2), */
#include <stdlib.h>    /* atoi */
#include <unistd.h>    /* symlink(2), symlinkat(2), readlink(2), lstat(2), unlink(2), unlinkat(2)*/
#include <string.h>    /* str*, strrchr, strcat, strcpy, strncpy, strncmp */
#include <sys/types.h> /* lstat(2), */
#include <sys/stat.h>  /* lstat(2), */
#include <errno.h>     /* E*, */
#include <limits.h>    /* PATH_MAX, */
#include <ctype.h>     /* isdigit, */
#include <fcntl.h>
#include <sys/syscall.h>

#include "cli/note.h"
#include "extension/extension.h"
#include "tracee/tracee.h"
#include "tracee/mem.h"
#include "tracee/statx.h"
#include "syscall/syscall.h"
#include "syscall/sysnum.h"
#include "path/path.h"
#include "path/f2fs-bug.h"
#include "arch.h"
#include "attribute.h"

#define PREFIX L2S_PREFIX
#define DELETED_SUFFIX " (deleted)"

#ifndef RENAME_NOREPLACE
#define RENAME_NOREPLACE (1 << 0)
#endif

#ifndef RENAME_EXCHANGE
#define RENAME_EXCHANGE (1 << 1)
#endif

typedef struct {
	char *path;
	char *intermediate;
	Sysnum sysnum;
	dev_t device;
	ino_t inode;
} PendingRemoval;

/**
 * Copy the contents of the @symlink into @value (nul terminated).
 * This function returns -errno if an error occured, otherwise 0.
 */
static int my_readlink(const char symlink[PATH_MAX], char value[PATH_MAX])
{
	ssize_t size;

	size = readlink(symlink, value, PATH_MAX);
	if (size < 0)
		return -errno;
	if (size >= PATH_MAX)
		return -ENAMETOOLONG;
	value[size] = '\0';

	return 0;
}

static int rename_noreplace(const char *oldpath, const char *newpath)
{
	if (syscall(SYS_renameat2, AT_FDCWD, oldpath, AT_FDCWD, newpath, RENAME_NOREPLACE) < 0)
		return -errno;
	return 0;
}

static int rename_back(Tracee *tracee, const char *from, const char *to)
{
	int status;

	status = rename_noreplace(from, to);
	if (status < 0)
		return status;
	notify_extensions(tracee, LINK2SYMLINK_RENAME, (intptr_t) from, (intptr_t) to);
	return 0;
}

static int repoint_intermediate(const char *intermediate, const char *target)
{
	char temporary[PATH_MAX];
	int length;
	int status;

	length = snprintf(temporary, PATH_MAX, "%s.new", intermediate);
	if (length < 0 || length >= PATH_MAX)
		return -ENAMETOOLONG;
	if (symlink(target, temporary) < 0) {
		if (errno != EEXIST || unlink(temporary) < 0 || symlink(target, temporary) < 0)
			return -errno;
	}
	if (rename(temporary, intermediate) < 0) {
		status = -errno;
		unlink(temporary);
		return status;
	}
	return 0;
}

static void undo_first_link(Tracee *tracee, const char *original, const char *intermediate, const char *final)
{
	if (unlink(original) < 0)
		return;
	if (rename_back(tracee, final, original) == 0)
		unlink(intermediate);
	else
		symlink(intermediate, original);
}

static int has_count(const char *final, int length)
{
	int i;

	if (length < 4)
		return 0;
	for (i = length - 4; i < length; i++) {
		if (!isdigit((unsigned char) final[i]))
			return 0;
	}
	return 1;
}

static int lower_link_count(Tracee *tracee, const char intermediate[PATH_MAX])
{
	char final[PATH_MAX];
	char new_final[PATH_MAX];
	int status;
	int length;
	int link_count;

	status = my_readlink(intermediate, final);
	if (status < 0)
		return status;

	length = strlen(final);
	if (!has_count(final, length))
		return -EINVAL;
	link_count = atoi(final + length - 4) - 1;

	if (link_count > 0) {
		length = snprintf(new_final, PATH_MAX, "%.*s%04d", length - 4, final, link_count);
		if (length < 0 || length >= PATH_MAX)
			return -ENAMETOOLONG;

		status = rename_noreplace(final, new_final);
		if (status < 0)
			return status;
		status = notify_extensions(tracee, LINK2SYMLINK_RENAME, (intptr_t) final, (intptr_t) new_final);
		if (status < 0) {
			rename_noreplace(new_final, final);
			return status;
		}

		status = repoint_intermediate(intermediate, new_final);
		if (status < 0) {
			rename_back(tracee, new_final, final);
			return status;
		}
		return 0;
	}

	if (unlink(intermediate) < 0)
		return -errno;
	if (unlink(final) < 0)
		return -errno;
	return notify_extensions(tracee, LINK2SYMLINK_UNLINK, (intptr_t) final, 0);
}

/**
 * Move the path pointed to by @tracee's @sysarg to a new location,
 * symlink the original path to this new one, make @tracee's @sysarg
 * point to the new location.  This function returns -errno if an
 * error occured, otherwise 0.
 */
static int move_and_symlink_path(Tracee *tracee, Reg sysarg, Reg link_target_sysarg)
{
	char original[PATH_MAX];
	char intermediate[PATH_MAX];
	char new_intermediate[PATH_MAX];
	char final[PATH_MAX];
	char new_final[PATH_MAX];
	char destination[PATH_MAX];
	char * name;
	const char * l2s_directory;
	struct stat statl;
	ssize_t size;
	int status;
	int length;
	int link_count;
	int first_link = 1;
	int intermediate_suffix;

	/* Note: this path was already canonicalized.  */
	size = read_string(tracee, original, peek_reg(tracee, CURRENT, sysarg), PATH_MAX);
	if (size < 0)
		return size;
	if (size >= PATH_MAX)
		return -ENAMETOOLONG;

	/* Sanity check: directories can't be linked.  */
	status = lstat(original, &statl);
	if (status < 0)
		return errno > 0 ? -errno : -ENOENT;
	if (S_ISDIR(statl.st_mode))
		return -EPERM;

	/* Check if it is a symbolic link.  */
	if (S_ISLNK(statl.st_mode)) {
		/* get name */
		size = my_readlink(original, intermediate);
		if (size < 0)
			return size;

		name = strrchr(intermediate, '/');
		if (name == NULL)
			name = intermediate;
		else
			name++;

		if (strncmp(name, PREFIX, strlen(PREFIX)) == 0)
			first_link = 0;
	} else {
		/* compute new name */
		name = strrchr(original,'/');
		if (name == NULL)
			name = original;
		else
			name++;

		l2s_directory = getenv("PROOT_L2S_DIR");
		if (l2s_directory != NULL && l2s_directory[0]) {
			if (strlen(PREFIX) + strlen(l2s_directory) + (strlen(original) - strlen(name)) + 6 >= PATH_MAX)
				return -ENAMETOOLONG;

			strcpy(intermediate, l2s_directory);
			if (l2s_directory[strlen(l2s_directory) - 1] != '/') {
				strcat(intermediate, "/");
			}
		} else {
			if (strlen(PREFIX) + strlen(original) + 5 >= PATH_MAX)
				return -ENAMETOOLONG;

			strncpy(intermediate, original, strlen(original) - strlen(name));
			intermediate[strlen(original) - strlen(name)] = '\0';
		}
		strcat(intermediate, PREFIX);
		strcat(intermediate, name);
	}

	if (first_link) {
		for (intermediate_suffix = 1; intermediate_suffix <= 9999; intermediate_suffix++) {
			length = snprintf(new_intermediate, PATH_MAX, "%s%04d", intermediate, intermediate_suffix);
			if (length < 0 || length >= PATH_MAX)
				return -ENAMETOOLONG;
			length = snprintf(final, PATH_MAX, "%s.0002", new_intermediate);
			if (length < 0 || length >= PATH_MAX)
				return -ENAMETOOLONG;

			if (symlink(final, new_intermediate) < 0) {
				if (errno == EEXIST)
					continue;
				return -errno;
			}

			status = rename_noreplace(original, final);
			if (status == 0)
				break;
			unlink(new_intermediate);
			if (status != -EEXIST)
				return status;
		}
		if (intermediate_suffix > 9999)
			return -EMLINK;
		strcpy(intermediate, new_intermediate);

		status = notify_extensions(tracee, LINK2SYMLINK_RENAME, (intptr_t) original, (intptr_t) final);
		if (status < 0) {
			if (rename_noreplace(final, original) == 0)
				unlink(intermediate);
			return status;
		}

		if (symlink(intermediate, original) < 0) {
			status = -errno;
			if (rename_back(tracee, final, original) == 0)
				unlink(intermediate);
			return status;
		}
	} else {
		size = my_readlink(intermediate, final);
		if (size < 0)
			return size;

		length = strlen(final);
		if (!has_count(final, length))
			return -EINVAL;
		link_count = atoi(final + length - 4);
		if (link_count >= 9999)
			return -EMLINK;

		length = snprintf(new_final, PATH_MAX, "%.*s%04d", length - 4, final, link_count + 1);
		if (length < 0 || length >= PATH_MAX)
			return -ENAMETOOLONG;

		status = rename_noreplace(final, new_final);
		if (status < 0)
			return status;
		status = notify_extensions(tracee, LINK2SYMLINK_RENAME, (intptr_t) final, (intptr_t) new_final);
		if (status < 0) {
			rename_noreplace(new_final, final);
			return status;
		}

		status = repoint_intermediate(intermediate, new_final);
		if (status < 0) {
			rename_back(tracee, new_final, final);
			return status;
		}
	}

	/* Perform symlink() operation within PRoot.  */
	status = read_path(tracee, destination, peek_reg(tracee, CURRENT, link_target_sysarg));
	if (status >= 0) {
		status = symlink(intermediate, destination);
		if (status < 0) status = -errno;
	}
	if (status < 0) {
		if (first_link)
			undo_first_link(tracee, original, intermediate, final);
		else
			lower_link_count(tracee, intermediate);
		return status;
	}
	poke_reg(tracee, SYSARG_RESULT, 0);
	set_sysnum(tracee, PR_void);

	return 0;
}


static int read_chain(const char path[PATH_MAX], char intermediate[PATH_MAX], struct stat *statl)
{
	char final[PATH_MAX];
	char *name;

	if (lstat(path, statl) < 0 || !S_ISLNK(statl->st_mode))
		return 0;
	if (my_readlink(path, intermediate) < 0)
		return 0;

	name = strrchr(intermediate, '/');
	if (name == NULL)
		name = intermediate;
	else
		name++;
	if (strncmp(name, PREFIX, strlen(PREFIX)) != 0)
		return 0;

	return my_readlink(intermediate, final) == 0;
}

static void forget_removed_name(Extension *extension)
{
	PendingRemoval *pending = talloc_get_type(extension->config, PendingRemoval);

	if (pending == NULL)
		return;
	TALLOC_FREE(pending->path);
	TALLOC_FREE(pending->intermediate);
}

static void remember_removed_name(Extension *extension, Tracee *tracee, Reg sysarg)
{
	PendingRemoval *pending = talloc_get_type(extension->config, PendingRemoval);
	char path[PATH_MAX];
	char intermediate[PATH_MAX];
	struct stat statl;
	ssize_t size;

	if (pending == NULL)
		return;

	size = read_string(tracee, path, peek_reg(tracee, CURRENT, sysarg), PATH_MAX);
	if (size < 0 || size >= PATH_MAX)
		return;
	if (!read_chain(path, intermediate, &statl))
		return;

	pending->path = talloc_strdup(pending, path);
	pending->intermediate = talloc_strdup(pending, intermediate);
	if (pending->path == NULL || pending->intermediate == NULL) {
		VERBOSE(tracee, 1, "link2symlink: can't remember \"%s\"", path);
		forget_removed_name(extension);
		return;
	}
	pending->sysnum = get_sysnum(tracee, ORIGINAL);
	pending->device = statl.st_dev;
	pending->inode = statl.st_ino;
}

static void lower_removed_name(Extension *extension, Tracee *tracee)
{
	PendingRemoval *pending = talloc_get_type(extension->config, PendingRemoval);
	struct stat statl;
	int status;

	if (pending == NULL || pending->intermediate == NULL)
		return;

	if (get_sysnum(tracee, ORIGINAL) == pending->sysnum
	    && peek_reg(tracee, CURRENT, SYSARG_RESULT) == 0
	    && !(lstat(pending->path, &statl) == 0
		 && statl.st_dev == pending->device
		 && statl.st_ino == pending->inode)) {
		status = lower_link_count(tracee, pending->intermediate);
		if (status < 0)
			VERBOSE(tracee, 1, "link2symlink: can't lower the count of \"%s\": %s",
				pending->intermediate, strerror(-status));
	}

	forget_removed_name(extension);
}

/**
 * sizeof(struct stat) cut to contain only fields that are at same addresses
 * regardless of whenever tracee is 32-bit or 64-bit.
 *
 * This allows modification of following fields:
 * - st_dev
 * - st_mode
 * - st_nlink
 * - st_uid
 * - st_gid
 * - st_rdev
 * - st_size
 * - st_blksize
 * - st_blocks
 */
#define SIZEOF_RELEVANT_STRUCT_STAT 72

/**
 * Make it so fake hard links look like real hard link with respect to number of links and inode
 * This function returns -errno if an error occured, otherwise 0.
 */
static int handle_sysexit_end(Tracee *tracee)
{
	word_t sysnum;

	sysnum = get_sysnum(tracee, ORIGINAL);

	#ifdef USERLAND
		if ((get_sysnum(tracee, CURRENT) == PR_fstat) || (get_sysnum(tracee, CURRENT) == PR_fstat64))
			return 0;

		if (((sysnum == PR_fstat) || (sysnum == PR_fstat64)) && (get_sysnum(tracee, CURRENT) == PR_readlinkat))
			return 0;
	#endif

	switch (sysnum) {

	case PR_fstatat64:                 //int fstatat(int dirfd, const char *pathname, struct stat *buf, int flags);
	case PR_newfstatat:                //int fstatat(int dirfd, const char *pathname, struct stat *buf, int flags);
	case PR_stat64:                    //int stat(const char *path, struct stat *buf);
	case PR_lstat64:                   //int lstat(const char *path, struct stat *buf);
	case PR_fstat64:                   //int fstat(int fd, struct stat *buf);
	case PR_stat:                      //int stat(const char *path, struct stat *buf);
	case PR_lstat:                     //int lstat(const char *path, struct stat *buf);
	case PR_fstat: {                   //int fstat(int fd, struct stat *buf);
		word_t result;
		Reg sysarg_stat;
		Reg sysarg_path;
		int status;
		struct stat statl = {};
		ssize_t size;
		char original[PATH_MAX];
		char intermediate[PATH_MAX];
		char final[PATH_MAX];
		char * name;
		struct stat finalStat;

		/* Override only if it succeed.  */
		result = peek_reg(tracee, CURRENT, SYSARG_RESULT);
		if (result != 0)
			return 0;

		if (sysnum == PR_fstat64 || sysnum == PR_fstat) {
			#ifndef USERLAND
				status = readlink_proc_pid_fd(tracee->pid, peek_reg(tracee, MODIFIED, SYSARG_1), original);
				if (status < 0) {
					VERBOSE(tracee, 3, "link2symlink: readlink_proc_pid_fd failed, status=%d", status);
					return 0; // Don't alter syscall result
				}
				if (strlen(original) > strlen(DELETED_SUFFIX) &&
						strcmp(original + strlen(original) - strlen(DELETED_SUFFIX), DELETED_SUFFIX) == 0)
					original[strlen(original) - strlen(DELETED_SUFFIX)] = '\0';
			#endif
			#ifdef USERLAND
				size = read_string(tracee, original, peek_reg(tracee, CURRENT, SYSARG_2), PATH_MAX);
				if (size < 0)
					return size;
				if (size >= PATH_MAX)
					return -ENAMETOOLONG;
			#endif
		} else {
			if (sysnum == PR_fstatat64 || sysnum == PR_newfstatat)
				sysarg_path = SYSARG_2;
			else
				sysarg_path = SYSARG_1;
			size = read_string(tracee, original, peek_reg(tracee, MODIFIED, sysarg_path), PATH_MAX);
			if (size < 0)
				return size;
			if (size >= PATH_MAX)
				return -ENAMETOOLONG;
		}

		name = strrchr(original, '/');
		if (name == NULL)
			name = original;
		else
			name++;

		/* Check if it is a link */
		status = lstat(original, &statl);

		if (strncmp(name, PREFIX, strlen(PREFIX)) == 0) {
			if (S_ISLNK(statl.st_mode)) {
				strcpy(intermediate,original);
				goto intermediate_proc;
			} else {
				strcpy(final,original);
				goto final_proc;
			}
		}

		if (!S_ISLNK(statl.st_mode))
			return 0;

		size = my_readlink(original, intermediate);
		if (size < 0)
			return size;

		name = strrchr(intermediate, '/');
		if (name == NULL)
			name = intermediate;
		else
			name++;

		if (strncmp(name, PREFIX, strlen(PREFIX)) != 0)
			return 0;

		intermediate_proc: size = my_readlink(intermediate, final);
		if (size < 0)
			return size;

		final_proc: status = lstat(final,&finalStat);
		if (status < 0)
			return status;

		finalStat.st_nlink = atoi(final + strlen(final) - 4);

		/* Get the address of the 'stat' structure.  */
		if (sysnum == PR_fstatat64 || sysnum == PR_newfstatat)
			sysarg_stat = SYSARG_3;
		else
			sysarg_stat = SYSARG_2;

		#ifdef USERLAND
			/* Overwrite the stat struct with the correct number of "links". */
			read_data(tracee, &statl, peek_reg(tracee, ORIGINAL, sysarg_stat), sizeof(statl));
			finalStat.st_mode = statl.st_mode;
			finalStat.st_uid = statl.st_uid;
			finalStat.st_gid = statl.st_gid;
		#endif
		status = write_data(tracee, peek_reg(tracee, ORIGINAL,  sysarg_stat), &finalStat,
			is_32on64_mode(tracee) ? SIZEOF_RELEVANT_STRUCT_STAT : sizeof(finalStat));
		if (status < 0)
			return status;

		return 0;
	}

	default:
		return 0;
	}
}

static void link2symlink_handle_statx(struct statx_syscall_state *state)
{
	if (!(state->statx_buf.stx_mask & STATX_NLINK))
		return;

	const char *path_ending = strrchr(state->host_path, '/');
	if (NULL == path_ending)
		return;

	size_t ending_len = strlen(path_ending);
	if (ending_len < strlen(PREFIX) + 6) /* 6 = strlen("/") + strlen(".0002") */
		return;

	if (0 != strncmp(path_ending + 1, PREFIX, strlen(PREFIX)))
		return;

	if (path_ending[ending_len - 5] != '.')
		return;

	for (size_t i = 1; i <= 4; i++) {
		if (!isdigit(path_ending[ending_len - i]))
			return;
	}

	state->statx_buf.stx_nlink = atoi(&path_ending[ending_len - 4]);
}

/**
 * When @translated_path is a faked hard-link, replace it with the
 * point it (internally) points to.
 */
static void translated_path(Tracee *tracee, char translated_path[PATH_MAX])
{
	char path2[PATH_MAX];
	char path[PATH_MAX];
	char *component;
	int status;

	Sysnum sysnum = get_sysnum(tracee, ORIGINAL);
	if (   sysnum == PR_unlink
	    || sysnum == PR_unlinkat
	    || sysnum == PR_link
	    || sysnum == PR_linkat
	    || sysnum == PR_rename
	    || sysnum == PR_renameat
	    || sysnum == PR_renameat2
	    || sysnum == PR_readlink
	    || sysnum == PR_readlinkat
	    /* Do NOT dereference L2S chains for chown/utimes syscalls.
	     * On Android (untrusted_app SELinux domain) these operations on
	     * files deep in the app data dir return ENOENT (masqueraded EPERM).
	     * By skipping L2S dereferencing the kernel sees the L2S symlink
	     * itself; fake_id0 then forces the result to 0 so dpkg proceeds.
	     * (dpkg tolerates EPERM but aborts on ENOENT.) */
	    || sysnum == PR_lchown
	    || sysnum == PR_lchown32
	    || sysnum == PR_chown
	    || sysnum == PR_chown32
	    || sysnum == PR_fchown
	    || sysnum == PR_fchown32
	    || sysnum == PR_fchownat
	    || sysnum == PR_utimes
	    || sysnum == PR_utimensat
	    || sysnum == PR_futimesat
	    || sysnum == PR_utime) {
		return;
	}

	if (should_skip_file_access_due_to_f2fs_bug(tracee, translated_path))
		return;

	status = my_readlink(translated_path, path);
	if (status < 0)
		return;

	component = strrchr(path, '/');
	if (component == NULL)
		return;
	component++;

	if (strncmp(component, PREFIX, strlen(PREFIX)) != 0)
		return;

	status = my_readlink(path, path2);
	if (status < 0)
		return;

#if 0 /* Sanity check. */
	component = strrchr(path, '/');
	if (component == NULL)
		return;
	component++;

	if (strncmp(component, PREFIX, strlen(PREFIX)) != 0)
		return;
#endif

	strcpy(translated_path, path2);
	return;
}

/**
 * Handler for linkat(..., "/proc/X/fd/Y", ..., AT_SYMLINK_FOLLOW)
 *
 * Returns:
 *    1 if operation was handled successfully
 *      (Syscall should be marked as successful without further actions)
 *    0 if this wasn't linkat from /proc//fd
 *      (Caller should proceed with usual link2symlink)
 *   <0 if operation failed
 *      (Syscall should be marked as failed without further actions)
 */
static int handle_linkat_from_proc_fd(Tracee *tracee) {
	/* Read source path, return if it doesn't belong to /proc  */
	char proc_path[128];
	ssize_t size = read_string(tracee, proc_path, peek_reg(tracee, CURRENT, SYSARG_2), sizeof(proc_path));
	if (size <= 0 || size >= (ssize_t) sizeof(proc_path)) {
		return 0;
	}
	if (compare_paths(proc_path, "/proc") != PATH2_IS_PREFIX) {
		return 0;
	}

	/* Ensure provided path is symlink to " (deleted)" file  */
	char target_path[PATH_MAX] = {};
	int status = readlink(proc_path, target_path, sizeof(target_path));
	if (status < 10 || status >= (ssize_t) sizeof(target_path)) {
		return 0;
	}
	if (0 != memcmp(&target_path[status - 10], DELETED_SUFFIX, 10)) {
		return 0;
	}

	/* Read stats of source file, ensure it is regular file  */
	struct stat stats = {};
	if (0 != stat(proc_path, &stats)) {
		return 0;
	}
	if (!S_ISREG(stats.st_mode)) {
		return 0;
	}

	/* Read path of target file (already translated by proot)  */
	size = read_string(tracee, target_path, peek_reg(tracee, CURRENT, SYSARG_4), PATH_MAX);
	if (size < 0 || size >= (ssize_t) sizeof(target_path)) {
		return 0;
	}

	/* Open source file for reading  */
	int source_fd = open(proc_path, O_RDONLY);
	if (source_fd < 0) {
		return 0;
	}

	/* Point of no return, below we no longer are allowed to "return 0",
	 * any errors will be propagated to caller
	 *
	 * Delete target file (we'll be replacing it).
	 * Ignore result of unlink, file could or could not exist,
	 * we'll report failure of open though  */
	unlink(target_path);

	/* Open target file for writing  */
	int target_fd = open(target_path, O_WRONLY|O_CREAT|O_EXCL, stats.st_mode & 0777);
	if (target_fd < 0) {
		status = -errno;
		if (status >= 0)
			status = -EPERM;
		close(source_fd);
		return status;
	}

	/* Copy contents of file  */
	char buf[4096];
	int nread;
	while (0 != (nread = read(source_fd, buf, sizeof(buf)))) {
		if (nread < 0) {
			status = -errno;
			if (status >= 0)
				status = -EPERM;
			close(source_fd);
			close(target_fd);
			return status;
		}
		int pos = 0;
		while (pos < nread) {
			int nwrite = write(target_fd, buf + pos, nread - pos);
			if (nwrite <= 0) {
				status = -errno;
				if (status >= 0)
					status = -EPERM;
				close(source_fd);
				close(target_fd);
				return status;
			}
			pos += nwrite;
		}
	}

	/* Copy successful, nothing more to be done for this syscall  */
	close(source_fd);
	close(target_fd);
	return 1;
}

/**
 * Handler for this @extension.  It is triggered each time an @event
 * occurred.  See ExtensionEvent for the meaning of @data1 and @data2.
 */
int link2symlink_callback(Extension *extension, ExtensionEvent event,
			intptr_t data1, intptr_t data2 UNUSED)
{
	int status;

	switch (event) {
	case INITIALIZATION: {
		/* List of syscalls handled by this extensions.  */
		static FilteredSysnum filtered_sysnums[] = {
			{ PR_link,		FILTER_SYSEXIT },
			{ PR_linkat,		FILTER_SYSEXIT },
			{ PR_unlink,		FILTER_SYSEXIT },
			{ PR_unlinkat,		FILTER_SYSEXIT },
			{ PR_fstat,		FILTER_SYSEXIT },
			{ PR_fstat64,		FILTER_SYSEXIT },
			{ PR_fstatat64,		FILTER_SYSEXIT },
			{ PR_lstat,		FILTER_SYSEXIT },
			{ PR_lstat64,		FILTER_SYSEXIT },
			{ PR_newfstatat,	FILTER_SYSEXIT },
			{ PR_stat,		FILTER_SYSEXIT },
			{ PR_stat64,		FILTER_SYSEXIT },
			{ PR_rename,		FILTER_SYSEXIT },
			{ PR_renameat,		FILTER_SYSEXIT },
			{ PR_renameat2,		FILTER_SYSEXIT },
			FILTERED_SYSNUM_END,
		};
		extension->filtered_sysnums = filtered_sysnums;
		extension->config = talloc_zero(extension, PendingRemoval);
		return extension->config == NULL ? -1 : 0;
	}

	case INHERIT_PARENT:
		return 1;

	case INHERIT_CHILD:
		extension->config = talloc_zero(extension, PendingRemoval);
		return extension->config == NULL ? -1 : 0;

	case SYSCALL_ENTER_END: {
		Tracee *tracee = TRACEE(extension);

		forget_removed_name(extension);

		switch (get_sysnum(tracee, ORIGINAL)) {
		case PR_rename:
			if ((int) data1 >= 0)
				remember_removed_name(extension, tracee, SYSARG_2);
			break;

		case PR_renameat:
			if ((int) data1 >= 0)
				remember_removed_name(extension, tracee, SYSARG_4);
			break;

		case PR_renameat2:
			if ((int) data1 >= 0 && (peek_reg(tracee, CURRENT, SYSARG_5) & RENAME_EXCHANGE) == 0)
				remember_removed_name(extension, tracee, SYSARG_4);
			break;

		case PR_unlink:
			if ((int) data1 >= 0)
				remember_removed_name(extension, tracee, SYSARG_1);
			break;

		case PR_unlinkat:
			if ((int) data1 >= 0 && (peek_reg(tracee, CURRENT, SYSARG_3) & AT_REMOVEDIR) == 0)
				remember_removed_name(extension, tracee, SYSARG_2);
			break;

		case PR_link:
			/* Convert:
			 *
			 *     int link(const char *oldpath, const char *newpath);
			 *
			 * into:
			 *
			 *     int symlink(const char *oldpath, const char *newpath);
			 */

			status = move_and_symlink_path(tracee, SYSARG_1, SYSARG_2);
			if (status < 0)
				return status;

			break;

		case PR_linkat:
			/*
			 * Handle linkat(..., "/proc/X/fd/Y", ..., AT_SYMLINK_FOLLOW)
			 */
			if (peek_reg(tracee, CURRENT, SYSARG_5) & AT_SYMLINK_FOLLOW) {
				status = handle_linkat_from_proc_fd(tracee);
				if (status < 0)
					return status;
				if (status == 1) {
					set_sysnum(tracee, PR_void);
					poke_reg(tracee, SYSARG_RESULT, 0);
					return 0;
				}
			}

			/* Convert:
			 *
			 *     int linkat(int olddirfd, const char *oldpath,
			 *                int newdirfd, const char *newpath, int flags);
			 *
			 * into:
			 *
			 *     int symlink(const char *oldpath, const char *newpath);
			 *
			 * Note: PRoot has already canonicalized
			 * linkat() paths this way:
			 *
			 *   olddirfd + oldpath -> oldpath
			 *   newdirfd + newpath -> newpath
			 */

			status = move_and_symlink_path(tracee, SYSARG_2, SYSARG_4);
			if (status < 0)
				return status;

			break;

		default:
			break;
		}
		return 0;
	}

	case SYSCALL_EXIT_END: {
		Tracee *tracee = TRACEE(extension);

		lower_removed_name(extension, tracee);
		return handle_sysexit_end(tracee);
	}

	case TRANSLATED_PATH:
		translated_path(TRACEE(extension), (char *) data1);
		return 0;

	case STATX_SYSCALL:
		link2symlink_handle_statx((struct statx_syscall_state *) data1);
		return 0;

	default:
		return 0;
	}
}
