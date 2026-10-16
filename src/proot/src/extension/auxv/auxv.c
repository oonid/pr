/* -*- c-set-style: "K&R"; c-basic-offset: 8 -*-
 *
 * The loader rewrites the auxiliary vector on the initial stack
 * (AT_EXECFN, AT_PHDR, AT_ENTRY, AT_BASE, ...) and fake_id0 rewrites
 * the ids there, but the kernel keeps its own copy and serves that one
 * through /proc/<pid>/auxv and prctl(PR_GET_AUXV).  Programs reading
 * those (uutils coreutils via rustix, for AT_EXECFN) then see the
 * loader instead of themselves.  Serve the stack copy from both.
 */

#include <linux/auxvec.h>        /* AT_NULL, */
#include <limits.h>              /* INT_MAX, */
#include <linux/limits.h>        /* PATH_MAX, */
#include <stdbool.h>             /* bool, */
#include <stdio.h>               /* fopen, fwrite, */
#include <stdlib.h>              /* strtol, */
#include <string.h>              /* strlen, strcmp, strncmp, memset, */
#include <talloc.h>              /* talloc_*, */

#include "extension/extension.h"
#include "execve/execve.h"       /* LoadInfo, */
#include "execve/auxv.h"         /* fetch_elf_aux_vectors, */
#include "path/temp.h"           /* create_temp_file, */
#include "syscall/sysnum.h"      /* PR_*, */
#include "tracee/abi.h"          /* sizeof_word, */
#include "tracee/mem.h"          /* write_data, */
#include "tracee/reg.h"          /* peek_reg, poke_reg, */

#define PR_GET_AUXV 0x41555856

/**
 * Serialize @tracee's stack auxv like the kernel does: word pairs up to
 * and including AT_NULL.  Returns NULL when @tracee was not started by
 * the loader, otherwise a buffer of *@size bytes owned by @tracee->ctx.
 */
static void *stack_auxv(const Tracee *tracee, size_t *size)
{
	ElfAuxVector *vectors;
	size_t word_size = sizeof_word(tracee);
	size_t nb_vectors;
	uint8_t *buffer;
	size_t i;

	if (tracee->load_info == NULL || tracee->load_info->auxv_address == 0)
		return NULL;

	vectors = fetch_elf_aux_vectors(tracee, tracee->load_info->auxv_address);
	if (vectors == NULL)
		return NULL;

	nb_vectors = talloc_array_length(vectors);
	*size = nb_vectors * 2 * word_size;
	buffer = talloc_zero_size(tracee->ctx, *size);
	if (buffer == NULL)
		return NULL;

	for (i = 0; i < nb_vectors; i++) {
		if (word_size == sizeof(uint32_t)) {
			((uint32_t *) buffer)[2 * i]     = vectors[i].type;
			((uint32_t *) buffer)[2 * i + 1] = vectors[i].value;
		} else {
			((uint64_t *) buffer)[2 * i]     = vectors[i].type;
			((uint64_t *) buffer)[2 * i + 1] = vectors[i].value;
		}
	}

	return buffer;
}

/**
 * Redirect an open of "/proc/<pid>/auxv" for a tracee to a temporary
 * file holding that tracee's stack auxv.
 */
static void redirect_proc_auxv(Tracee *tracee, char path[PATH_MAX])
{
	const char *suffix = "/auxv";
	size_t suffix_len = strlen(suffix);
	size_t len = strlen(path);
	const Tracee *target;
	char *pid_end = NULL;
	const char *new_path;
	void *content;
	size_t size;
	FILE *file;
	bool written;
	long pid;

	if (len <= 6 + suffix_len || strncmp(path, "/proc/", 6) != 0
	    || strcmp(path + len - suffix_len, suffix) != 0)
		return;

	pid = strtol(path + 6, &pid_end, 10);
	if (pid_end != path + len - suffix_len || pid <= 0 || pid > INT_MAX)
		return;

	target = get_tracee(tracee, pid, false);
	if (target == NULL)
		return;

	content = stack_auxv(target, &size);
	if (content == NULL)
		return;

	new_path = create_temp_file(tracee->ctx, "auxv");
	if (new_path == NULL)
		return;

	file = fopen(new_path, "w");
	if (file == NULL)
		return;

	written = fwrite(content, size, 1, file) == 1;
	if (fclose(file) == 0 && written)
		strncpy(path, new_path, PATH_MAX);
}

/**
 * Overwrite the buffer filled by a successful prctl(PR_GET_AUXV).  The
 * result (the size of the kernel's copy) is kept: the stack auxv has
 * the same records, only different values.
 */
static void rewrite_pr_get_auxv(Tracee *tracee)
{
	word_t result = peek_reg(tracee, CURRENT, SYSARG_RESULT);
	word_t address = peek_reg(tracee, ORIGINAL, SYSARG_2);
	word_t length = peek_reg(tracee, ORIGINAL, SYSARG_3);
	uint8_t *content;
	uint8_t *padded;
	size_t size;
	size_t copied;

	if ((long) result <= 0 || address == 0 || length == 0)
		return;

	content = stack_auxv(tracee, &size);
	if (content == NULL)
		return;

	copied = length < result ? length : result;
	padded = talloc_zero_size(tracee->ctx, copied);
	if (padded == NULL)
		return;
	memcpy(padded, content, size < copied ? size : copied);

	(void) write_data(tracee, address, padded, copied);
}

static FilteredSysnum filtered_sysnums[] = {
	{ PR_prctl, FILTER_SYSEXIT },
	FILTERED_SYSNUM_END,
};

int auxv_callback(Extension *extension, ExtensionEvent event,
		intptr_t data1, intptr_t data2 UNUSED)
{
	Tracee *tracee;
	Sysnum sysnum;

	switch (event) {
	case INITIALIZATION:
		extension->filtered_sysnums = filtered_sysnums;
		return 0;

	case TRANSLATED_PATH:
		tracee = TRACEE(extension);
		sysnum = get_sysnum(tracee, ORIGINAL);
		if (sysnum == PR_open || sysnum == PR_openat)
			redirect_proc_auxv(tracee, (char *) data1);
		return 0;

	case SYSCALL_EXIT_END:
		tracee = TRACEE(extension);
		if (get_sysnum(tracee, ORIGINAL) == PR_prctl
		    && peek_reg(tracee, ORIGINAL, SYSARG_1) == PR_GET_AUXV)
			rewrite_pr_get_auxv(tracee);
		return 0;

	default:
		return 0;
	}
}
