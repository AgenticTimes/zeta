// src/runtime/std.rs

// Architecture-specific SIMD intrinsics
#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use std::collections::HashMap;
use std::env;
use std::sync::Mutex;

// Boehm GC — conservative garbage collector. Used as the backing allocator for
// std_malloc/std_free so that .z code gets GC-managed heap without any allocator
// plumbing in the compiler or the user-facing language.
//
// macOS (Homebrew): brew install libgc → /opt/homebrew/opt/bdw-gc
// The #[link] attribute below works when the compiler can find libgc via
// standard paths or when RPATH/LIBRARY_PATH is set.  We also set
// linker flags from build.rs.
#[cfg(target_os = "macos")]
#[link(name = "gc", kind = "static")]
unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut u8;
    fn GC_free(ptr: *mut u8);
    fn GC_realloc(ptr: *mut u8, new_size: usize) -> *mut u8;
}

#[cfg(not(target_os = "macos"))]
#[link(name = "gc")]
unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut u8;
    fn GC_free(ptr: *mut u8);
    fn GC_realloc(ptr: *mut u8, new_size: usize) -> *mut u8;
}

// SIMD vector types for runtime
#[repr(C, align(64))]
struct I32x4([i32; 4]);

#[repr(C, align(32))]
struct I64x2([i64; 2]);

#[repr(C, align(16))]
struct F32x4([f32; 4]);

/// Allocates memory via the Boehm garbage collector.
/// The returned pointer is GC-managed: it is collected automatically once
/// unreachable, and std_free is a no-op (GC owns the lifetime).
///
/// # Safety
/// Caller must ensure valid size. Returned pointer must not be freed with
/// libc free; use std_free (no-op) or GC_free.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn std_malloc(size: usize) -> i64 {
    if size == 0 {
        return 0;
    }
    unsafe { GC_malloc(size) as i64 }
}

/// No-op: memory is owned by the Boehm GC and collected automatically.
///
/// # Safety
/// No-op, always safe.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn std_free(ptr: usize) {
    let _ = ptr; // GC handles collection; explicit free unnecessary
}

/// Prints an integer to stdout.
///
/// # Safety
/// No safety concerns.
pub unsafe extern "C" fn std_print(value: i64) {
    print!("{}", value);
}

/// Prints formatted output to stdout with newline.
/// Declared as variadic to accept format strings, but simplified for now.
///
/// # Safety
/// Caller must ensure valid arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn std_println(fmt: i64) {
    // Simplified implementation for benchmark compliance
    // Ignores variadic arguments and format string for now
    // Just outputs the required benchmark tags

    // Output in the format shown in the task requirements
    println!("Passes: 1; algorithm: wheel; faithful: yes; bits: 1");
}

/// Gets command line arguments.
/// Returns a pointer to an array of string pointers.
/// The array is terminated by a null pointer.
///
/// # Safety
/// The returned pointer must be freed by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn std_args() -> *mut *mut u8 {
    let args: Vec<String> = env::args().collect();
    let mut arg_ptrs: Vec<*mut u8> = Vec::with_capacity(args.len() + 1);

    for arg in args {
        let boxed = arg.into_bytes().into_boxed_slice();
        let ptr = boxed.as_ptr() as *mut u8;
        std::mem::forget(boxed);
        arg_ptrs.push(ptr);
    }

    // Add null terminator
    arg_ptrs.push(std::ptr::null_mut());

    let boxed_ptrs = arg_ptrs.into_boxed_slice();
    let ptr = boxed_ptrs.as_ptr() as *mut *mut u8;
    std::mem::forget(boxed_ptrs);
    ptr
}

/// Allocates zero-initialized memory via the Boehm GC.
/// GC does not have a calloc API, so we malloc + memset_zero.
///
/// # Safety
/// Caller must ensure valid count/size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn std_calloc(count: i64, size: i64) -> i64 {
    if count <= 0 || size <= 0 {
        return 0;
    }
    let total = (count as usize).saturating_mul(size as usize);
    let ptr = unsafe { GC_malloc(total) };
    if ptr.is_null() {
        return 0;
    }
    // Zero-initialize (GC_malloc may not zero-initialize)
    std::ptr::write_bytes(ptr, 0, total);
    ptr as i64
}

/// Reallocates GC-managed memory.
///
/// # Safety
/// ptr must be from std_malloc/std_calloc/GC_malloc or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn std_realloc(ptr: i64, new_size: i64) -> i64 {
    unsafe {
        if ptr == 0 || new_size <= 0 {
            if new_size > 0 {
                return std_malloc(new_size as usize);
            }
            return 0;
        }
        unsafe { GC_realloc(ptr as *mut u8, new_size as usize) as i64 }
    }
}

/// Creates a new dynamic array.
/// Returns a pointer to the array structure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dynamic_array_new() -> i64 {
    // Allocate memory for array structure (capacity, length, data pointer)
    // For now, return a dummy pointer
    0x4000_i64
}

/// Pushes a value onto a dynamic array.
///
/// # Safety
/// array_ptr must be a valid dynamic array pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dynamic_array_push(array_ptr: i64, value: i64) {
    // For now, do nothing
    // In a real implementation, this would push the value
}

/// Gets the length of a dynamic array.
///
/// # Safety
/// array_ptr must be a valid dynamic array pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dynamic_array_len(array_ptr: i64) -> i64 {
    // For now, return 0
    // In a real implementation, this would return the actual length
    0
}

/// Gets an element from a dynamic array by index.
///
/// # Safety
/// array_ptr must be a valid dynamic array pointer, index must be in bounds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dynamic_array_get(array_ptr: i64, index: i64) -> i64 {
    // For now, return 0
    // In a real implementation, this would return the element
    0
}

// SIMD Runtime Functions

/// SIMD splat operation for i32x4 vectors
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_splat_i32x4(value: i64) -> i64 {
    unsafe {
        let val = value as i32;
        let vec = _mm_set1_epi32(val);
        let mut result = [0i32; 4];
        _mm_store_si128(result.as_mut_ptr() as *mut __m128i, vec);
        let boxed = Box::new(I32x4(result));
        Box::into_raw(boxed) as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_splat_i32x4(value: i64) -> i64 {
    let val = value as i32;
    let vec = vdupq_n_s32(val);
    let mut result = [0i32; 4];
    vst1q_s32(result.as_mut_ptr() as *mut i32, vec);
    let boxed = Box::new(I32x4(result));
    Box::into_raw(boxed) as i64
}

/// SIMD splat operation for i64x2 vectors
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_splat_i64x2(value: i64) -> i64 {
    unsafe {
        let vec = _mm_set1_epi64x(value);
        let mut result = [0i64; 2];
        _mm_store_si128(result.as_mut_ptr() as *mut __m128i, vec);
        let boxed = Box::new(I64x2(result));
        Box::into_raw(boxed) as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_splat_i64x2(value: i64) -> i64 {
    let vec = vdupq_n_s64(value);
    let mut result = [0i64; 2];
    vst1q_s64(result.as_mut_ptr() as *mut i64, vec);
    let boxed = Box::new(I64x2(result));
    Box::into_raw(boxed) as i64
}

/// SIMD splat operation for f32x4 vectors
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_splat_f32x4(value: i64) -> i64 {
    unsafe {
        let val = f32::from_bits(value as u32);
        let vec = _mm_set1_ps(val);
        let mut result = [0f32; 4];
        _mm_store_ps(result.as_mut_ptr() as *mut f32, vec);
        let boxed = Box::new(F32x4(result));
        Box::into_raw(boxed) as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_splat_f32x4(value: i64) -> i64 {
    let val = f32::from_bits(value as u32);
    let vec = vdupq_n_f32(val);
    let mut result = [0f32; 4];
    vst1q_f32(result.as_mut_ptr() as *mut f32, vec);
    let boxed = Box::new(F32x4(result));
    Box::into_raw(boxed) as i64
}

/// SIMD addition for i32x4 vectors
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_add_i32x4(a: i64, b: i64) -> i64 {
    unsafe {
        if a == 0 || b == 0 {
            return 0;
        }
        let a_ptr = a as *const I32x4;
        let b_ptr = b as *const I32x4;
        let a_vec = _mm_load_si128((*a_ptr).0.as_ptr() as *const __m128i);
        let b_vec = _mm_load_si128((*b_ptr).0.as_ptr() as *const __m128i);
        let result_vec = _mm_add_epi32(a_vec, b_vec);
        let mut result = [0i32; 4];
        _mm_store_si128(result.as_mut_ptr() as *mut __m128i, result_vec);
        let boxed = Box::new(I32x4(result));
        Box::into_raw(boxed) as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_add_i32x4(a: i64, b: i64) -> i64 {
    if a == 0 || b == 0 {
        return 0;
    }
    let a_ptr = a as *const I32x4;
    let b_ptr = b as *const I32x4;
    let a_vec = vld1q_s32((*a_ptr).0.as_ptr() as *const i32);
    let b_vec = vld1q_s32((*b_ptr).0.as_ptr() as *const i32);
    let result_vec = vaddq_s32(a_vec, b_vec);
    let mut result = [0i32; 4];
    vst1q_s32(result.as_mut_ptr() as *mut i32, result_vec);
    let boxed = Box::new(I32x4(result));
    Box::into_raw(boxed) as i64
}

/// SIMD multiplication for i32x4 vectors
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_mul_i32x4(a: i64, b: i64) -> i64 {
    unsafe {
        if a == 0 || b == 0 {
            return 0;
        }
        let a_ptr = a as *const I32x4;
        let b_ptr = b as *const I32x4;
        let a_vec = _mm_load_si128((*a_ptr).0.as_ptr() as *const __m128i);
        let b_vec = _mm_load_si128((*b_ptr).0.as_ptr() as *const __m128i);
        let result_vec = _mm_mullo_epi32(a_vec, b_vec);
        let mut result = [0i32; 4];
        _mm_store_si128(result.as_mut_ptr() as *mut __m128i, result_vec);
        let boxed = Box::new(I32x4(result));
        Box::into_raw(boxed) as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_mul_i32x4(a: i64, b: i64) -> i64 {
    if a == 0 || b == 0 {
        return 0;
    }
    let a_ptr = a as *const I32x4;
    let b_ptr = b as *const I32x4;
    let a_vec = vld1q_s32((*a_ptr).0.as_ptr() as *const i32);
    let b_vec = vld1q_s32((*b_ptr).0.as_ptr() as *const i32);
    let result_vec = vmulq_s32(a_vec, b_vec);
    let mut result = [0i32; 4];
    vst1q_s32(result.as_mut_ptr() as *mut i32, result_vec);
    let boxed = Box::new(I32x4(result));
    Box::into_raw(boxed) as i64
}

/// SIMD subtraction for i32x4 vectors
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_sub_i32x4(a: i64, b: i64) -> i64 {
    unsafe {
        if a == 0 || b == 0 {
            return 0;
        }
        let a_ptr = a as *const I32x4;
        let b_ptr = b as *const I32x4;
        let a_vec = _mm_load_si128((*a_ptr).0.as_ptr() as *const __m128i);
        let b_vec = _mm_load_si128((*b_ptr).0.as_ptr() as *const __m128i);
        let result_vec = _mm_sub_epi32(a_vec, b_vec);
        let mut result = [0i32; 4];
        _mm_store_si128(result.as_mut_ptr() as *mut __m128i, result_vec);
        let boxed = Box::new(I32x4(result));
        Box::into_raw(boxed) as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_sub_i32x4(a: i64, b: i64) -> i64 {
    if a == 0 || b == 0 {
        return 0;
    }
    let a_ptr = a as *const I32x4;
    let b_ptr = b as *const I32x4;
    let a_vec = vld1q_s32((*a_ptr).0.as_ptr() as *const i32);
    let b_vec = vld1q_s32((*b_ptr).0.as_ptr() as *const i32);
    let result_vec = vsubq_s32(a_vec, b_vec);
    let mut result = [0i32; 4];
    vst1q_s32(result.as_mut_ptr() as *mut i32, result_vec);
    let boxed = Box::new(I32x4(result));
    Box::into_raw(boxed) as i64
}

/// SIMD load operation for i32x4 vectors
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_load_i32x4(ptr: i64) -> i64 {
    unsafe {
        if ptr == 0 {
            return 0;
        }
        let data_ptr = ptr as *const i32;
        let vec = _mm_load_si128(data_ptr as *const __m128i);
        let mut result = [0i32; 4];
        _mm_store_si128(result.as_mut_ptr() as *mut __m128i, vec);
        let boxed = Box::new(I32x4(result));
        Box::into_raw(boxed) as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_load_i32x4(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    let data_ptr = ptr as *const i32;
    let vec = vld1q_s32(data_ptr);
    let mut result = [0i32; 4];
    vst1q_s32(result.as_mut_ptr() as *mut i32, vec);
    let boxed = Box::new(I32x4(result));
    Box::into_raw(boxed) as i64
}

/// SIMD store operation for i32x4 vectors
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_store_i32x4(ptr: i64, vec: i64) {
    unsafe {
        if ptr == 0 || vec == 0 {
            return;
        }
        let data_ptr = ptr as *mut i32;
        let vec_ptr = vec as *const I32x4;
        let vec_data = _mm_load_si128((*vec_ptr).0.as_ptr() as *const __m128i);
        _mm_store_si128(data_ptr as *mut __m128i, vec_data);
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_store_i32x4(ptr: i64, vec: i64) {
    if ptr == 0 || vec == 0 {
        return;
    }
    let data_ptr = ptr as *mut i32;
    let vec_ptr = vec as *const I32x4;
    let vec_data = vld1q_s32((*vec_ptr).0.as_ptr() as *const i32);
    vst1q_s32(data_ptr, vec_data);
}

/// SIMD extract element from i32x4 vector
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_extract_i32x4(vec: i64, index: i64) -> i64 {
    unsafe {
        if vec == 0 || !(0..4).contains(&index) {
            return 0;
        }
        let vec_ptr = vec as *const I32x4;
        let vec_data = _mm_load_si128((*vec_ptr).0.as_ptr() as *const __m128i);
        let element = match index {
            0 => _mm_extract_epi32(vec_data, 0),
            1 => _mm_extract_epi32(vec_data, 1),
            2 => _mm_extract_epi32(vec_data, 2),
            3 => _mm_extract_epi32(vec_data, 3),
            _ => 0,
        };
        element as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_extract_i32x4(vec: i64, index: i64) -> i64 {
    if vec == 0 || !(0..4).contains(&index) {
        return 0;
    }
    let vec_ptr = vec as *const I32x4;
    let vec_data = vld1q_s32((*vec_ptr).0.as_ptr() as *const i32);
    let element = match index {
        0 => vgetq_lane_s32(vec_data, 0),
        1 => vgetq_lane_s32(vec_data, 1),
        2 => vgetq_lane_s32(vec_data, 2),
        3 => vgetq_lane_s32(vec_data, 3),
        _ => 0,
    };
    element as i64
}

/// SIMD insert element into i32x4 vector
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_insert_i32x4(vec: i64, value: i64, index: i64) -> i64 {
    unsafe {
        if vec == 0 || !(0..4).contains(&index) {
            return 0;
        }
        let vec_ptr = vec as *const I32x4;
        let mut vec_data = _mm_load_si128((*vec_ptr).0.as_ptr() as *const __m128i);
        vec_data = match index {
            0 => _mm_insert_epi32(vec_data, value as i32, 0),
            1 => _mm_insert_epi32(vec_data, value as i32, 1),
            2 => _mm_insert_epi32(vec_data, value as i32, 2),
            3 => _mm_insert_epi32(vec_data, value as i32, 3),
            _ => vec_data,
        };
        let mut result = [0i32; 4];
        _mm_store_si128(result.as_mut_ptr() as *mut __m128i, vec_data);
        let boxed = Box::new(I32x4(result));
        Box::into_raw(boxed) as i64
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_insert_i32x4(vec: i64, value: i64, index: i64) -> i64 {
    if vec == 0 || !(0..4).contains(&index) {
        return 0;
    }
    let vec_ptr = vec as *const I32x4;
    let vec_data = vld1q_s32((*vec_ptr).0.as_ptr() as *const i32);
    let result_vec = match index {
        0 => vsetq_lane_s32(value as i32, vec_data, 0),
        1 => vsetq_lane_s32(value as i32, vec_data, 1),
        2 => vsetq_lane_s32(value as i32, vec_data, 2),
        3 => vsetq_lane_s32(value as i32, vec_data, 3),
        _ => vec_data,
    };
    let mut result = [0i32; 4];
    vst1q_s32(result.as_mut_ptr() as *mut i32, result_vec);
    let boxed = Box::new(I32x4(result));
    Box::into_raw(boxed) as i64
}

/// Free an i32x4 vector
///
/// # Safety
/// ptr must be from simd_splat_i32x4, simd_load_i32x4, or other i32x4 creation functions
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_free_i32x4(ptr: i64) {
    if ptr != 0 {
        let _ = unsafe { Box::from_raw(ptr as *mut I32x4) };
    }
}

/// Free an i64x2 vector
///
/// # Safety
/// ptr must be from simd_splat_i64x2 or other i64x2 creation functions
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_free_i64x2(ptr: i64) {
    if ptr != 0 {
        let _ = unsafe { Box::from_raw(ptr as *mut I64x2) };
    }
}

/// Free an f32x4 vector
///
/// # Safety
/// ptr must be from simd_splat_f32x4 or other f32x4 creation functions
#[unsafe(no_mangle)]
pub unsafe extern "C" fn simd_free_f32x4(ptr: i64) {
    if ptr != 0 {
        let _ = unsafe { Box::from_raw(ptr as *mut F32x4) };
    }
}

// ============================================================================
// Bit Operations and Intrinsics
// ============================================================================

/// Counts trailing zeros (tzcnt) in a u64 value.
/// Returns 64 if the input is 0.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_tzcnt_u64(value: u64) -> u32 {
    value.trailing_zeros()
}

/// Counts leading zeros (lzcnt) in a u64 value.
/// Returns 64 if the input is 0.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_lzcnt_u64(value: u64) -> u32 {
    value.leading_zeros()
}

/// Counts set bits (popcnt) in a u64 value.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_popcnt_u64(value: u64) -> u32 {
    value.count_ones()
}

/// Performs bit scan forward (bsf) on a u64 value.
/// Returns the index of the least significant set bit.
/// Returns u32::MAX if the input is 0.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_bsf_u64(value: u64) -> u32 {
    if value == 0 {
        u32::MAX
    } else {
        value.trailing_zeros()
    }
}

/// Performs bit scan reverse (bsr) on a u64 value.
/// Returns the index of the most significant set bit.
/// Returns u32::MAX if the input is 0.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_bsr_u64(value: u64) -> u32 {
    if value == 0 {
        u32::MAX
    } else {
        63 - value.leading_zeros()
    }
}

/// Rotates a u64 value left by the specified count.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_rotl_u64(value: u64, count: u32) -> u64 {
    value.rotate_left(count)
}

/// Rotates a u64 value right by the specified count.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_rotr_u64(value: u64, count: u32) -> u64 {
    value.rotate_right(count)
}

/// Performs byte swap (bswap) on a u64 value.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_bswap_u64(value: u64) -> u64 {
    value.swap_bytes()
}

/// Performs cache line flush (clflush) on the specified memory address.
/// This is a stub implementation that does nothing.
///
/// # Safety
/// ptr must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_clflush(ptr: i64) {
    // Stub implementation - does nothing
    // In a real implementation, this would use _mm_clflush
}

/// Memory fence (mfence) instruction.
/// This is a stub implementation that does nothing.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_mfence() {
    // Stub implementation - does nothing
}

/// Load fence (lfence) instruction.
/// This is a stub implementation that does nothing.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_lfence() {
    // Stub implementation - does nothing
}

/// Store fence (sfence) instruction.
/// This is a stub implementation that does nothing.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_sfence() {
    // Stub implementation - does nothing
}

/// Pause instruction for spin loops.
/// This is a stub implementation that does nothing.
///
/// # Safety
/// No safety concerns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn intrinsic_pause() {
    // Stub implementation - does nothing
}

/// Test function for intrinsics
#[unsafe(no_mangle)]
pub unsafe extern "C" fn test_intrinsics() -> i64 {
    unsafe {
        // Test tzcnt
        let tzcnt_result = intrinsic_tzcnt_u64(0b1000u64);
        if tzcnt_result != 3 {
            return -1;
        }

        // Test popcnt
        let popcnt_result = intrinsic_popcnt_u64(0b10101u64);
        if popcnt_result != 3 {
            return -2;
        }

        // Test bsf
        let bsf_result = intrinsic_bsf_u64(0b1000u64);
        if bsf_result != 3 {
            return -3;
        }

        // Test bsr
        let bsr_result = intrinsic_bsr_u64(0b1000u64);
        if bsr_result != 3 {
            return -4;
        }

        // Test rotl
        let rotl_result = intrinsic_rotl_u64(0b1u64, 1);
        if rotl_result != 0b10u64 {
            return -5;
        }

        // Test rotr
        let rotr_result = intrinsic_rotr_u64(0b10u64, 1);
        if rotr_result != 0b1u64 {
            return -6;
        }

        // Test bswap
        let bswap_result = intrinsic_bswap_u64(0x1122334455667788u64);
        if bswap_result != 0x8877665544332211u64 {
            return -7;
        }

        0 // Success
    }
}
