// SPDX-License-Identifier: GPL-2.0-or-later
/* Surfaces and flips: a flip shows a surface in the viewer, and completes when the viewer has moved on. */
#include "standin.h"

void flip_done(struct work_struct *w)
{
	struct NvKmsKapiDevice *d = container_of(w, struct NvKmsKapiDevice, flip_work);
	struct NvKmsKapiEvent e = { .type = NVKMS_EVENT_TYPE_FLIP_OCCURRED, .device = d, .privateData = d->private_data };

	/*
	 * A flip completing is what frees the frame it replaced, for a client or the compositor to draw into. As on a real
	 * display that has to wait until nothing is reading it, here the viewer. A ticking viewer has taken the new frame by
	 * then, so flips go at its monitor's rate, one a tick. The limit is for a viewer that has gone.
	 */
	unsigned long limit = jiffies + msecs_to_jiffies(100);
	u64 newest = d->flip_number;

	while (time_before(jiffies, limit)) {
		u64 reading = nvfwd_show_reading();

		if (d->viewer ? reading >= newest : !d->outgoing || reading != d->outgoing)
			break;
		usleep_range(200, 400);
	}
	while (atomic_add_unless(&d->flips, -1, 0))
		d->event(&e);
}

static void flip_event_at_vblank(struct NvKmsKapiDevice *d)
{
	u64 now = ktime_get_ns();

	atomic_inc(&d->flips);
	hrtimer_start(&d->flip_timer, ns_to_ktime(d->viewer ? now : now - now % d->period_ns + d->period_ns), HRTIMER_MODE_ABS);
}

static void surface_flip(struct NvKmsKapiDevice *d, struct NvKmsKapiSurface *s);

void held_done(struct work_struct *w)
{
	struct NvKmsKapiDevice *d = container_of(w, struct NvKmsKapiDevice, held_work);
	struct NvKmsKapiSurface *s = NULL;
	bool event = false;

	spin_lock_irq(&d->held_lock);
	if (d->held && atomic_read(&d->semaphores[d->held_index]) == SEM_READY) {
		atomic_set(&d->semaphores[d->held_index], SEM_IDLE);
		s = d->held;
		event = d->held_event;
		d->held = NULL;
	}
	spin_unlock_irq(&d->held_lock);
	if (s)
		surface_flip(d, s);
	if (event)
		flip_event_at_vblank(d);
}

/* Once a second: is a viewer ticking, and at what rate? A change of rate is a change of mode, for the compositor's pacing. */
void rate_check(struct work_struct *w)
{
	struct NvKmsKapiDevice *d = container_of(to_delayed_work(w), struct NvKmsKapiDevice, rate_work);
	u64 rate = nvfwd_show_rate();
	u32 period = (u32)rate, ticks = rate >> 32;

	d->viewer = follow_viewer && period && ticks != d->viewer_ticks;
	d->viewer_ticks = ticks;
	if (d->viewer && abs((s64)period - (s64)d->period_ns) * 100 > d->period_ns) {
		struct NvKmsKapiEvent e = { .type = NVKMS_EVENT_TYPE_DPY_CHANGED, .device = d, .privateData = d->private_data };

		d->period_ns = period;
		e.u.displayChanged.display = DISPLAY_HANDLE;
		d->event(&e);
	}
	schedule_delayed_work(&d->rate_work, HZ);
}

enum hrtimer_restart flip_timer_done(struct hrtimer *t)
{
	schedule_work(&container_of(t, struct NvKmsKapiDevice, flip_timer)->flip_work);
	return HRTIMER_NORESTART;
}

struct NvKmsKapiSurface *createSurface(struct NvKmsKapiDevice *d, struct NvKmsKapiCreateSurfaceParams *params)
{
	struct NvKmsKapiSurface *s = kzalloc(sizeof(*s), GFP_KERNEL);

	if (!s)
		return NULL;
	s->memory = params->planes[0].memory;
	s->width = params->width;
	s->height = params->height;
	s->pitch = params->planes[0].pitch;
	s->layout = params->explicit_layout ? params->layout : s->memory->layout.layout;
	s->gobs_y = params->explicit_layout ? params->log2GobsPerBlockY : s->memory->layout.blockLinear.log2GobsPerBlock.y;
	switch (params->format) {
	case NvKmsSurfaceMemoryFormatA8R8G8B8:
		s->fourcc = FOURCC_AR24;
		break;
	case NvKmsSurfaceMemoryFormatA8B8G8R8:
		s->fourcc = FOURCC_AB24;
		break;
	case NvKmsSurfaceMemoryFormatX8B8G8R8:
		s->fourcc = FOURCC_XB24;
		break;
	default:
		s->fourcc = FOURCC_XR24;
	}
	pr_info("nvidia-modeset stand-in: surface %ux%u pitch %u format %d explicit %d layout %d\n", s->width, s->height, s->pitch, params->format,
		params->explicit_layout, params->layout);
	return s;
}

void destroySurface(struct NvKmsKapiDevice *d, struct NvKmsKapiSurface *s)
{
	spin_lock_irq(&d->held_lock);
	if (d->held == s)
		d->held = NULL;
	spin_unlock_irq(&d->held_lock);
	flush_work(&d->held_work);
	kfree(s);
}

/* A free id, or the longest held: the viewer replaces what it had under an id, and a frame is never left unshown. */
static u32 show_id_take(struct NvKmsKapiDevice *d, struct NvKmsKapiMemory *m)
{
	u32 id = d->next_shown++ % NVFWD_SHOW_MAX_SURFACES;

	spin_lock_irq(&d->held_lock);
	if (d->shown[id])
		d->shown[id]->show_id = 0;
	d->shown[id] = m;
	m->show_id = id + 1;
	spin_unlock_irq(&d->held_lock);
	return id;
}

static void surface_flip(struct NvKmsKapiDevice *d, struct NvKmsKapiSurface *s)
{
	struct NvKmsKapiMemory *m = s->memory;

	if (!m->show_id) {
		long rc = memory_export_file(d, m);
		u32 id = show_id_take(d, m);

		if (!rc)
			rc = nvfwd_show_surface(s->memory->export_id, NVFWD_SURFACE_ID(id, s->layout, s->memory->layout.blockLinear.genericMemory, s->gobs_y,
								       s->memory->layout.blockLinear.pitchInBlocks), s->width, s->height, s->pitch, s->fourcc, PAGE_ALIGN(s->memory->size));
		if (rc) {
			pr_warn_ratelimited("nvidia-modeset stand-in: show surface: %ld\n", rc);
			return;
		}
	}
	d->outgoing = d->flip_number;
	nvfwd_show_flip(m->show_id - 1, ++d->flip_number);
	if (log_flips)
		pr_info("nvidia-modeset stand-in: flip %llu committed %llu shown %llu\n", d->flip_number, d->committed_ns, ktime_get_real_ns());
}

NvBool applyModeSetConfig(struct NvKmsKapiDevice *d, const struct NvKmsKapiRequestedModeSetConfig *request,
				 struct NvKmsKapiModeSetReplyConfig *reply, const NvBool commit)
{
	const struct NvKmsKapiHeadRequestedConfig *head = &request->headRequestedConfig[0];
	struct NvKmsKapiSurface *surface;
	const struct NvKmsKapiSyncParams *sync;

	if (!(request->headsMask & 1))
		return NV_TRUE;
	if (head->modeSetConfig.bActive && (head->modeSetConfig.mode.timings.hVisible != WIDTH || head->modeSetConfig.mode.timings.vVisible != HEIGHT))
		return NV_FALSE;
	if (!commit)
		return NV_TRUE;
	surface = head->layerRequestedConfig[0].config.surface;
	d->committed_ns = ktime_get_real_ns();
	sync = &head->layerRequestedConfig[0].config.syncParams;
	memset(reply, 0, sizeof(*reply));
	if (head->modeSetConfig.bActive && surface && sync->semaphoreSpecified) {
		bool event = d->had_surface;

		spin_lock_irq(&d->held_lock);
		if (d->held) {
			/* A newer frame takes its place. Its semaphore still fires, and its flip must still be answered. */
			atomic_cmpxchg(&d->semaphores[d->held_index], SEM_WAITING, SEM_ABANDONED);
			atomic_cmpxchg(&d->semaphores[d->held_index], SEM_READY, SEM_IDLE);
			if (d->held_event)
				flip_event_at_vblank(d);
		}
		d->held = surface;
		d->held_index = sync->u.semaphore.index;
		d->held_event = event;
		spin_unlock_irq(&d->held_lock);
		d->had_surface = true;
		schedule_work(&d->held_work); /* it may have fired before this commit got here */
		return NV_TRUE;
	}
	if (head->modeSetConfig.bActive && surface)
		surface_flip(d, surface);
	/* nvidia-drm expects an event only when the head was already active with a framebuffer: __will_generate_flip_event. */
	if (d->had_surface) {
		flip_event_at_vblank(d);
	}
	d->had_surface = head->modeSetConfig.bActive && surface;
	return NV_TRUE;
}

NvBool getFlipPendingStatus(const struct NvKmsKapiDevice *d, const NvU32 head, const NvU32 layer, NvBool *pending)
{
	*pending = NV_FALSE;
	return NV_TRUE;
}
