// Local fallible allocator adapter; applied only to the encoder's build copies.
#pragma once
#include <cstddef>
#include <new>
#include <limits>
#include <type_traits>
void* unityrs_astc_allocate(std::size_t size, std::size_t alignment);
void unityrs_astc_release(const void* pointer) noexcept;
void unityrs_astc_destructor(void* pointer, void (*destroy)(void*)) noexcept;
template<class T> T* unityrs_astc_new() {
    void* p = unityrs_astc_allocate(sizeof(T), alignof(T));
    T* value = new (p) T;
    unityrs_astc_destructor(value, [](void* owned) { static_cast<T*>(owned)->~T(); });
    return value;
}
template<class T> T* unityrs_astc_array(std::size_t count) {
    static_assert(std::is_trivially_destructible<T>::value, "tracked arrays must be trivial");
    if (count > std::numeric_limits<std::size_t>::max() / sizeof(T)) throw std::bad_alloc();
    return static_cast<T*>(unityrs_astc_allocate(count * sizeof(T), alignof(T)));
}
template<class T> void unityrs_astc_delete(T* p) noexcept {
    unityrs_astc_release(p);
}
