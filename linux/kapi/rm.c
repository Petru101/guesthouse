// SPDX-License-Identifier: GPL-2.0-or-later
/* The RM calls of the stand-in's own client. */
#include <linux/module.h>
#include "standin.h"

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("A stand-in for nvidia-modeset in a guest");

long rm_control(struct NvKmsKapiDevice *d, u32 object, u32 cmd, void *params, u32 size, s32 fd_off, u32 fd_id)
{
	NVOS54_PARAMETERS p = { .hClient = d->client, .hObject = object, .cmd = cmd, .paramsSize = size };
	struct nvfwd_krm a = { d->ctl, CTL_ESC(ESC_RM_CONTROL, sizeof(p)), &p, sizeof(p), offsetof(NVOS54_PARAMETERS, params), params, size, -1, fd_off, fd_id };
	long rc = nvfwd_rm(&a);

	return rc ? rc : p.status;
}

long rm_alloc(struct NvKmsKapiDevice *d, u32 parent, u32 *handle, u32 hclass, void *params, u32 size)
{
	NVOS21_PARAMETERS p = { .hRoot = d->client, .hObjectParent = parent, .hObjectNew = *handle, .hClass = hclass, .paramsSize = size };
	struct nvfwd_krm a = { d->ctl, CTL_ESC(ESC_RM_ALLOC, sizeof(p)), &p, sizeof(p), offsetof(NVOS21_PARAMETERS, pAllocParms), params, size, -1, -1, 0 };
	long rc = nvfwd_rm(&a);

	*handle = p.hObjectNew;
	return rc ? rc : p.status;
}

void rm_free(struct NvKmsKapiDevice *d, u32 parent, u32 handle)
{
	NVOS00_PARAMETERS p = { .hRoot = d->client, .hObjectParent = parent, .hObjectOld = handle };
	struct nvfwd_krm a = { d->ctl, CTL_ESC(ESC_RM_FREE, sizeof(p)), &p, sizeof(p), 0, NULL, 0, -1, -1, 0 };

	nvfwd_rm(&a);
}
