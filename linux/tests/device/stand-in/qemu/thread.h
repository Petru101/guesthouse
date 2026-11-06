/* QEMU's mutex over pthreads. One thread uses it here, so this only has to compile and not deadlock. */
#ifndef STAND_IN_THREAD_H
#define STAND_IN_THREAD_H

#include <pthread.h>

typedef pthread_mutex_t QemuMutex;
#define qemu_mutex_init(m) pthread_mutex_init((m), NULL)
#define qemu_mutex_lock(m) pthread_mutex_lock(m)
#define qemu_mutex_unlock(m) pthread_mutex_unlock(m)

#endif
