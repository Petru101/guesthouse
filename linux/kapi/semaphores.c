// SPDX-License-Identifier: GPL-2.0-or-later
#include "standin.h"

/*
 * Semaphore surfaces, which is what gives the user-mode driver sync fds. The object and its values live on the host,
 * and the value is plain memory we can map, so waiters are watched here by reading it. RM's own REGISTER_WAITER takes
 * a kernel callback pointer, which means nothing to the host, so it is not forwarded and needs no call back to us.
 */
struct NvKmsKapiSemaphoreSurface {
	struct NvKmsKapiDevice *d;
	u32 handle;
	struct NvKmsKapiMemory *sem, *max;
	u8 *sem_map, *max_map;
	atomic_t firing; /* waiters taken off the list and not finished yet */
};

struct NvKmsKapiSemaphoreSurfaceCallback {
	struct list_head link;
	struct NvKmsKapiSemaphoreSurface *s;
	NvKmsSemaphoreSurfaceCallbackProc *proc;
	void *data;
	u64 index, wait, value;
	unsigned long since;
};

static LIST_HEAD(waiters);
static DEFINE_SPINLOCK(waiters_lock);
static DECLARE_WAIT_QUEUE_HEAD(waiters_wake);
struct task_struct *waiters_thread;

/* As nvidia-drm reads it: with a max submitted value the semaphore is 32 bits and that gives the top half. */
static u64 semaphore_value(struct NvKmsKapiSemaphoreSurface *s, u64 index)
{
	u8 *sem = s->sem_map + index * s->d->semsurf_stride;
	u64 max, value;

	if (!s->max_map)
		return READ_ONCE(*(volatile u64 *)sem);
	max = READ_ONCE(*(volatile u64 *)(s->max_map + index * s->d->semsurf_stride + s->d->semsurf_max_offset));
	value = READ_ONCE(*(volatile u32 *)sem); /* after the max submitted read, as there */
	if ((max & 0xffffffffull) < value)
		max -= 0x100000000ull;
	return value | (max & 0xffffffff00000000ull);
}

NvBool setSemaphore(struct NvKmsKapiDevice *d, struct NvKmsKapiSemaphoreSurface *s, NvU64 index, NvU64 value)
{
	NV_SEMAPHORE_SURFACE_CTRL_SET_VALUE_PARAMS p = { .index = index, .newValue = value };

	long rc = rm_control(d, s->handle, NV_SEMAPHORE_SURFACE_CTRL_CMD_SET_VALUE, &p, sizeof(p), -1, 0);

	if (rc)
		pr_warn_ratelimited("nvidia-modeset stand-in: semaphore set %llu at %llu: %lx\n", value, index, rc);
	return !rc;
}

int waiters_main(void *unused)
{
	while (!kthread_should_stop()) {
		struct NvKmsKapiSemaphoreSurfaceCallback *w, *next;
		LIST_HEAD(done);

		wait_event_interruptible(waiters_wake, !list_empty(&waiters) || kthread_should_stop());
		spin_lock(&waiters_lock);
		list_for_each_entry_safe(w, next, &waiters, link)
			if (semaphore_value(w->s, w->index) >= w->wait) {
				atomic_inc(&w->s->firing);
				list_move_tail(&w->link, &done);
			} else if (time_after(jiffies, w->since + 2 * HZ)) {
				w->since = jiffies;
				pr_warn("nvidia-modeset stand-in: waiter at %llu for %llu, now %llu, then %llu, callback %d\n", w->index, w->wait,
					semaphore_value(w->s, w->index), w->value, !!w->proc);
			}
		spin_unlock(&waiters_lock);
		list_for_each_entry_safe(w, next, &done, link) {
			struct NvKmsKapiSemaphoreSurface *s = w->s;

			list_del(&w->link);
			if (w->value)
				setSemaphore(s->d, s, w->index, w->value);
			if (w->proc)
				w->proc(w->data);
			kfree(w);
			atomic_dec(&s->firing);
			wake_up(&waiters_wake);
		}
		usleep_range(50, 100);
	}
	return 0;
}

static void semaphore_memory_free(struct NvKmsKapiDevice *d, struct NvKmsKapiMemory *m, void *map)
{
	if (map)
		unmapMemory(d, m, NVKMS_KAPI_MAPPING_TYPE_KERNEL, map);
	if (m->mapped)
		nvfwd_kunmap(m->window, m->size);
	rm_free(d, HANDLE_DEVICE, m->handle);
	kfree(m);
}

void freeSemaphoreSurface(struct NvKmsKapiDevice *d, struct NvKmsKapiSemaphoreSurface *s)
{
	struct NvKmsKapiSemaphoreSurfaceCallback *w, *next;

	if (!s)
		return;
	/* Nothing may read this surface once it goes. RM drops its listeners with the object as well. */
	spin_lock(&waiters_lock);
	list_for_each_entry_safe(w, next, &waiters, link)
		if (w->s == s) {
			list_del(&w->link);
			kfree(w);
		}
	spin_unlock(&waiters_lock);
	if (current != waiters_thread)
		wait_event(waiters_wake, !atomic_read(&s->firing));
	if (s->max && s->max != s->sem)
		semaphore_memory_free(d, s->max, s->max_map);
	if (s->sem)
		semaphore_memory_free(d, s->sem, s->sem_map);
	rm_free(d, HANDLE_SUBDEVICE, s->handle);
	kfree(s);
}

static struct NvKmsKapiMemory *semaphore_memory(struct NvKmsKapiDevice *d, u32 handle, u64 size, void **map)
{
	struct NvKmsKapiMemory *m = kzalloc(sizeof(*m), GFP_KERNEL);

	if (!m)
		return NULL;
	refcount_set(&m->refs, 1);
	m->handle = handle;
	m->size = size;
	m->sysmem = 1; /* RM keeps semaphore surfaces there */
	if (!mapMemory(d, m, NVKMS_KAPI_MAPPING_TYPE_KERNEL, map))
		*map = NULL;
	return m;
}

struct NvKmsKapiSemaphoreSurface *importSemaphoreSurface(struct NvKmsKapiDevice *d, NvU64 user, NvU64 user_size, void **sem_map, void **max_map)
{
	struct NvKmsKapiPrivImportSemaphoreSurfaceParams p;
	NV_SEMAPHORE_SURFACE_CTRL_REF_MEMORY_PARAMS ref = { 0 };
	struct NvKmsKapiSemaphoreSurface *s;
	NVOS55_PARAMETERS dup = { 0 };
	struct nvfwd_krm a = { d->ctl, CTL_ESC(ESC_RM_DUP_OBJECT, sizeof(dup)), &dup, sizeof(dup), 0, NULL, 0, -1, -1, 0 };
	long rc;

	if (user_size != sizeof(p) || copy_from_user(&p, u64_to_user_ptr(user), sizeof(p)))
		return NULL;
	s = kzalloc(sizeof(*s), GFP_KERNEL);
	if (!s)
		return NULL;
	s->d = d;
	dup.hClient = d->client;
	dup.hParent = HANDLE_SUBDEVICE;
	dup.hObject = __sync_fetch_and_add(&d->next_handle, 1);
	dup.hClientSrc = p.hClient;
	dup.hObjectSrc = p.hSemaphoreSurface;
	rc = nvfwd_rm(&a);
	if (rc || dup.status) {
		pr_warn("nvidia-modeset stand-in: semaphore dup: %ld %x\n", rc, dup.status);
		kfree(s);
		return NULL;
	}
	s->handle = dup.hObject;
	rc = rm_control(d, s->handle, NV_SEMAPHORE_SURFACE_CTRL_CMD_REF_MEMORY, &ref, sizeof(ref), -1, 0);
	if (rc) {
		pr_warn("nvidia-modeset stand-in: semaphore memory: %lx\n", rc);
		goto fail;
	}
	s->sem = semaphore_memory(d, ref.hSemaphoreMem, p.semaphoreSurfaceSize, (void **)&s->sem_map);
	if (!s->sem || !s->sem_map)
		goto fail;
	if (ref.hMaxSubmittedMem == ref.hSemaphoreMem) {
		s->max = s->sem;
		s->max_map = s->sem_map;
	} else if (ref.hMaxSubmittedMem) {
		s->max = semaphore_memory(d, ref.hMaxSubmittedMem, p.semaphoreSurfaceSize, (void **)&s->max_map);
		if (!s->max || !s->max_map)
			goto fail;
	}
	*sem_map = s->sem_map;
	*max_map = s->max_map;
	return s;
fail:
	freeSemaphoreSurface(d, s);
	return NULL;
}

NvKmsKapiRegisterWaiterResult registerSemaphore(struct NvKmsKapiDevice *d, struct NvKmsKapiSemaphoreSurface *s, NvKmsSemaphoreSurfaceCallbackProc *proc,
						       void *data, NvU64 index, NvU64 wait, NvU64 value, struct NvKmsKapiSemaphoreSurfaceCallback **h)
{
	struct NvKmsKapiSemaphoreSurfaceCallback *w;

	if (!s || (!proc && !value) || (value && value <= wait)) {
		pr_warn_ratelimited("nvidia-modeset stand-in: bad waiter at %llu: wait %llu value %llu\n", index, wait, value);
		return NVKMS_KAPI_REG_WAITER_FAILED;
	}
	w = kzalloc(sizeof(*w), GFP_KERNEL);
	if (!w)
		return NVKMS_KAPI_REG_WAITER_FAILED;
	w->s = s;
	w->proc = proc;
	w->data = data;
	w->index = index;
	w->wait = wait;
	w->value = value;
	w->since = jiffies;
	spin_lock(&waiters_lock);
	if (semaphore_value(s, index) >= wait) {
		spin_unlock(&waiters_lock);
		kfree(w);
		/* As RM does: the update runs at once, and only a caller wanting a notification is told. */
		if (value && !setSemaphore(d, s, index, value))
			return NVKMS_KAPI_REG_WAITER_FAILED;
		return proc ? NVKMS_KAPI_REG_WAITER_ALREADY_SIGNALLED : NVKMS_KAPI_REG_WAITER_SUCCESS;
	}
	list_add_tail(&w->link, &waiters);
	if (h && proc)
		*h = w;
	spin_unlock(&waiters_lock);
	wake_up(&waiters_wake);
	return NVKMS_KAPI_REG_WAITER_SUCCESS;
}

/* False if it has already fired or is firing, which is what the caller needs to know. */
NvBool unregisterSemaphore(struct NvKmsKapiDevice *d, struct NvKmsKapiSemaphoreSurface *s, NvU64 index, NvU64 wait, struct NvKmsKapiSemaphoreSurfaceCallback *h)
{
	struct NvKmsKapiSemaphoreSurfaceCallback *w, *found = NULL;

	spin_lock(&waiters_lock);
	list_for_each_entry(w, &waiters, link)
		if (w == h && w->s == s && w->index == index && w->wait == wait) {
			list_del(&w->link);
			found = w;
			break;
		}
	spin_unlock(&waiters_lock);
	kfree(found);
	return found != NULL;
}
