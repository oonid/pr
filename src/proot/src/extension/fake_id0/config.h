#ifndef FAKE_ID0_CONFIG_H
#define FAKE_ID0_CONFIG_H

#include <sys/types.h>   /* uid_t, gid_t */
#include <unistd.h>      /* getuid, getgid */

typedef struct {
	uid_t ruid;
	uid_t euid;
	uid_t suid;
	uid_t fsuid;

	gid_t rgid;
	gid_t egid;
	gid_t sgid;
	gid_t fsgid;

	mode_t umask;

	gid_t app_cache_gid;
} Config;

static inline uid_t fake_owner_uid(const Config *config, uid_t uid)
{
	return uid == getuid() ? config->euid : uid;
}

static inline gid_t fake_owner_gid(const Config *config, gid_t gid)
{
	return gid == getgid() || gid == config->app_cache_gid ? config->egid : gid;
}

#endif /* FAKE_ID0_CONFIG_H */
