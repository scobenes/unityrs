// Linux-only regression hook: refuse pthread_create after graph admission.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>

static atomic_int refuse;
static atomic_int attempts;

void unityrs_refuse_threads(int enabled) {
    atomic_store(&refuse, enabled);
}

int unityrs_thread_attempts(void) {
    return atomic_load(&attempts);
}

int pthread_create(pthread_t *thread, const pthread_attr_t *attr,
                   void *(*start)(void *), void *arg) {
    if (atomic_load(&refuse)) {
        atomic_fetch_add(&attempts, 1);
        return EAGAIN;
    }
    int (*real_create)(pthread_t *, const pthread_attr_t *,
                       void *(*)(void *), void *) = dlsym(RTLD_NEXT, "pthread_create");
    return real_create(thread, attr, start, arg);
}
