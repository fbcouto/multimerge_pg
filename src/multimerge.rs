use rayon::prelude::*;

// ==========================================
// PHASE 0: O(1) ENTROPY SHIELD
// ==========================================

/// Ultra-cheap heuristic (samples ~100 elements near the middle).
/// Decides whether to trace blocks or if the data is pure noise/chaos.
fn evaluate_local_entropy<T: Ord>(arr: &[T]) -> bool {
    let n = arr.len();
    if n < 120 {
        return false;
    }
    let mid = n / 2;
    let mut direction_changes = 0;
    let mut is_ascending = arr[mid] <= arr[mid + 1];
    
    for i in (mid + 1)..(mid + 100).min(n - 1) {
        let current_direction = arr[i] <= arr[i + 1];
        if current_direction != is_ascending {
            direction_changes += 1;
            is_ascending = current_direction;
        }
    }
    // If direction changes > 15 times in 100 elements, it's pure chaos.
    direction_changes > 15
}

// ==========================================
// PHASE 1: FRACTAL DETECTION (L1 CACHE)
// ==========================================

pub fn detect_global_trend<T: Ord + Sync>(arr: &[T]) -> Vec<i64> {
    let n = arr.len();
    if n <= 1 {
        return if n == 1 { vec![1] } else { Vec::new() };
    }

    // Level 1: Macro-blocks of 32,768 (Advancing 32,767 for 1 overlapping element)
    let macro_slice_len = 32_768;
    let macro_step = macro_slice_len - 1;

    if n <= macro_slice_len {
        return process_macro_block(arr);
    }

    let num_macro_blocks = (n + macro_step - 1) / macro_step;

    (0..num_macro_blocks)
        .into_par_iter()
        .map(|i| {
            let start = i * macro_step;
            let end = std::cmp::min(start + macro_slice_len, n);
            process_macro_block(&arr[start..end])
        })
        .reduce(
            || Vec::new(),
            |left, right| merge_metadata_pure(left, right),
        )
}

fn process_macro_block<T: Ord + Sync>(arr: &[T]) -> Vec<i64> {
    let n = arr.len();
    
    // Validated Champion Size for L1 Cache
    let micro_slice_len = 512;
    let micro_step = micro_slice_len - 1;

    if n <= micro_slice_len {
        return generate_sequential_metadata(arr);
    }

    let num_micro_blocks = (n + micro_step - 1) / micro_step;

    (0..num_micro_blocks)
        .into_par_iter()
        .map(|i| {
            let start = i * micro_step;
            let end = std::cmp::min(start + micro_slice_len, n);
            generate_sequential_metadata(&arr[start..end])
        })
        .reduce(
            || Vec::new(),
            |left, right| merge_metadata_pure(left, right),
        )
}

/// Pure metadata stitching without reading the array, O(log N) in parallel via Reduce.
fn merge_metadata_pure(mut left: Vec<i64>, right: Vec<i64>) -> Vec<i64> {
    if left.is_empty() { return right; }
    if right.is_empty() { return left; }

    let last_left = left.pop().unwrap();
    let first_right = right[0];

    // SINGLETON RULE
    if last_left.unsigned_abs() == 1 {
        left.extend_from_slice(&right);
        return left;
    }
    if first_right.unsigned_abs() == 1 {
        left.push(last_left);
        left.extend_from_slice(&right[1..]);
        return left;
    }

    let left_asc = last_left > 0;
    let right_asc = first_right > 0;

    if left_asc == right_asc {
        let sign = if left_asc { 1 } else { -1 };
        let combined_mag = last_left.unsigned_abs() + first_right.unsigned_abs() - 1;
        left.push(combined_mag as i64 * sign);
        left.extend_from_slice(&right[1..]);
    } else {
        left.push(last_left);
        let right_mag = first_right.unsigned_abs() - 1;
        let sign = if right_asc { 1 } else { -1 };
        left.push(right_mag as i64 * sign);
        left.extend_from_slice(&right[1..]);
    }

    left
}

fn generate_sequential_metadata<T: Ord>(arr: &[T]) -> Vec<i64> {
    let n = arr.len();
    if n == 0 { return Vec::new(); }
    if n == 1 { return vec![1]; }

    let mut metadata = Vec::with_capacity(n / 64);
    let mut head = 0;

    while head < n - 1 {
        let mut tail = head + 1;
        if arr[head] <= arr[tail] {
            while tail < n && arr[tail - 1] <= arr[tail] { tail += 1; }
            metadata.push((tail - head) as i64);
        } else {
            while tail < n && arr[tail - 1] > arr[tail] { tail += 1; }
            metadata.push(-((tail - head) as i64));
        }
        head = tail;
    }
    if head == n - 1 {
        metadata.push(1);
    }
    metadata
}

// ==========================================
// HYBRID CO-RANK + BIDIRECTIONAL MERGE
// ==========================================

fn co_rank<T: Ord>(k: usize, a: &[T], b: &[T]) -> (usize, usize) {
    let m = a.len();
    let n = b.len();
    let mut i_lo = k.saturating_sub(n);
    let mut i_hi = k.min(m);
    while i_lo < i_hi {
        let i = i_lo + (i_hi - i_lo + 1) / 2;
        let j = k - i;
        if j == n || a[i - 1] <= b[j] {
            i_lo = i;
        } else {
            i_hi = i - 1;
        }
    }
    (i_lo, k - i_lo)
}

#[inline]
fn move_elem<T: Clone>(dest: &mut T, src: &mut T) {
    if std::mem::needs_drop::<T>() {
        std::mem::swap(dest, src);
    } else {
        *dest = src.clone();
    }
}

#[inline]
fn move_slice<T: Clone>(dest: &mut [T], src: &mut [T]) {
    if std::mem::needs_drop::<T>() {
        dest.swap_with_slice(src);
    } else {
        dest.clone_from_slice(src);
    }
}

fn merge_seq<T: Ord + Clone>(a: &mut [T], b: &mut [T], dest: &mut [T]) {
    let (mut i, mut j, mut k) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] <= b[j] {
            move_elem(&mut dest[k], &mut a[i]);
            i += 1;
        } else {
            move_elem(&mut dest[k], &mut b[j]);
            j += 1;
        }
        k += 1;
    }
    if i < a.len() { move_slice(&mut dest[k..], &mut a[i..]); }
    if j < b.len() { move_slice(&mut dest[k..], &mut b[j..]); }
}

fn bidirectional_merge<T: Ord + Clone + Send + Sync>(a: &mut [T], b: &mut [T], dest: &mut [T], leaf_size: usize) {
    if a.is_empty() {
        move_slice(dest, b);
        return;
    }
    if b.is_empty() {
        move_slice(dest, a);
        return;
    }
    let total = a.len() + b.len();
    if total <= leaf_size {
        merge_seq(a, b, dest);
        return;
    }

    let k = (total + 1) / 2;
    let front_count = k - 1;

    let (pa, pb) = co_rank(front_count, a, b);
    let (qa, qb) = co_rank(front_count + 2, a, b);

    let (a_front, a_rest) = a.split_at_mut(pa);
    let (a_mid, a_back) = a_rest.split_at_mut(qa - pa);
    let (b_front, b_rest) = b.split_at_mut(pb);
    let (b_mid, b_back) = b_rest.split_at_mut(qb - pb);

    let (dest_front, rest) = dest.split_at_mut(front_count);
    let (dest_middle, dest_back) = rest.split_at_mut(2);

    rayon::join(
        || merge_seq(a_front, b_front, dest_front),
        || merge_seq(a_back, b_back, dest_back),
    );
    merge_seq(a_mid, b_mid, dest_middle);
}

const CORANK_SPLIT_FACTOR: usize = 16;

fn parallel_merge<T: Ord + Clone + Send + Sync>(a: &mut [T], b: &mut [T], dest: &mut [T], leaf_size: usize) {
    let total = a.len() + b.len();
    if total > leaf_size.saturating_mul(CORANK_SPLIT_FACTOR) {
        let k = total / 2;
        let (i, j) = co_rank(k, a, b);
        let (a_l, a_r) = a.split_at_mut(i);
        let (b_l, b_r) = b.split_at_mut(j);
        let (dest_left, dest_right) = dest.split_at_mut(k);
        rayon::join(
            || parallel_merge(a_l, b_l, dest_left, leaf_size),
            || parallel_merge(a_r, b_r, dest_right, leaf_size),
        );
        return;
    }
    bidirectional_merge(a, b, dest, leaf_size);
}

fn get_leaf_size<T>() -> usize {
    const L1_CACHE_BYTES: usize = 32_768;
    let element_size = std::mem::size_of::<T>().max(1);
    (L1_CACHE_BYTES / element_size).clamp(4096, 8192)
}

// ==========================================
// STRUCTURED PATH: BOTTOM-UP MERGE
// ==========================================

fn block_offsets(metadata: &[i64]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(metadata.len() + 1);
    let mut off = 0usize;
    offsets.push(0);
    for &m in metadata {
        off += m.unsigned_abs() as usize;
        offsets.push(off);
    }
    offsets
}

fn bottom_up_merge<T: Ord + Clone + Send + Sync>(
    v: &mut [T],
    buf: &mut [T],
    metadata: &[i64],
    offsets: &[usize],
    leaf_size: usize,
    into_buf: bool,
) {
    let num_blocks = metadata.len();

    if num_blocks == 1 {
        let is_desc = metadata[0] < 0;
        let n = v.len();
        if into_buf {
            if is_desc {
                for i in 0..n {
                    move_elem(&mut buf[i], &mut v[n - 1 - i]);
                }
            } else {
                move_slice(buf, v);
            }
        } else if is_desc {
            parallel_reverse(v);
        }
        return;
    }

    let base = offsets[0];
    let split_idx = num_blocks / 2;
    let mid = offsets[split_idx] - base;
    let (left_meta, right_meta) = metadata.split_at(split_idx);
    let left_offsets = &offsets[..=split_idx];
    let right_offsets = &offsets[split_idx..];

    {
        let (v_l, v_r) = v.split_at_mut(mid);
        let (buf_l, buf_r) = buf.split_at_mut(mid);
        rayon::join(
            || bottom_up_merge(v_l, buf_l, left_meta, left_offsets, leaf_size, !into_buf),
            || bottom_up_merge(v_r, buf_r, right_meta, right_offsets, leaf_size, !into_buf),
        );
    }

    if into_buf {
        if v[mid - 1] <= v[mid] {
            move_slice(buf, v);
        } else {
            let (v_l, v_r) = v.split_at_mut(mid);
            parallel_merge(v_l, v_r, buf, leaf_size);
        }
    } else {
        if buf[mid - 1] <= buf[mid] {
            move_slice(v, buf);
        } else {
            let (buf_l, buf_r) = buf.split_at_mut(mid);
            parallel_merge(buf_l, buf_r, v, leaf_size);
        }
    }
}

// ==========================================
// PARALLEL REVERSE (Rayon)
// ==========================================
fn parallel_reverse<T: Send + Sync>(arr: &mut [T]) {
    let n = arr.len();
    if n <= 100_000 {
        arr.reverse();
        return;
    }
    
    let mid = n / 2;
    let (left, right) = arr.split_at_mut(mid);
    
    left.par_iter_mut()
        .zip(right.par_iter_mut().rev())
        .for_each(|(a, b)| std::mem::swap(a, b));
}

// ==========================================
// MAIN ENTRY POINT (Specialized for Copy Types)
// ==========================================

/// High-performance relational sort. 
/// Requires T: Copy to elide memory zero-initialization on the buffer.
pub fn multi_merge_sort<T: Ord + Copy + Send + Sync>(arr: &mut [T]) {
    let n = arr.len();
    let leaf_size = get_leaf_size::<T>();

    if n <= leaf_size {
        arr.sort();
        return;
    }

    if evaluate_local_entropy(arr) {
        arr.par_sort();
        return;
    }

    let metadata = detect_global_trend(arr);

    if metadata.len() == 1 {
        if metadata[0] > 0 {
            return;
        }
        parallel_reverse(arr);
        return;
    }

    let offsets = block_offsets(&metadata);
    let mut buffer: Vec<T> = Vec::with_capacity(n);
    
    // SAFETY: T: Copy guarantees that any bit pattern is valid. 
    // Leaving this memory uninitialized is safe because bottom_up_merge 
    // overwrites every position before any logical read happens.
    unsafe { buffer.set_len(n); }
    
    bottom_up_merge(arr, &mut buffer, &metadata, &offsets, leaf_size, false);
}

// ==========================================
// TESTS
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    // Modified to derive Copy to comply with the new optimized routine.
    #[derive(Clone, Copy, Debug)]
    struct Keyed {
        key: u32,
        idx: u32,
    }

    impl PartialEq for Keyed { fn eq(&self, other: &Self) -> bool { self.key == other.key } }
    impl Eq for Keyed {}
    impl PartialOrd for Keyed { fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) } }
    impl Ord for Keyed { fn cmp(&self, other: &Self) -> std::cmp::Ordering { self.key.cmp(&other.key) } }

    fn is_sorted(v: &[Keyed]) -> bool { v.windows(2).all(|w| w[0].key <= w[1].key) }

    fn is_stable(v: &[Keyed]) -> bool {
        let mut i = 0;
        while i < v.len() {
            let mut j = i;
            while j + 1 < v.len() && v[j + 1].key == v[i].key { j += 1; }
            for k in i..j {
                if v[k].idx > v[k + 1].idx { return false; }
            }
            i = j + 1;
        }
        true
    }

    #[test]
    fn block_offsets_computes_correct_boundaries() {
        let metadata = vec![3, -2, 5, -1, 4];
        let offsets = block_offsets(&metadata);
        assert_eq!(offsets, vec![0, 3, 5, 10, 11, 15]);
    }

    #[test]
    fn bottom_up_merge_matches_reference_across_structured_trials() {
        let leaf_size = 4;
        let mut rng = StdRng::seed_from_u64(13);
        for trial in 0..500 {
            let n: usize = rng.gen_range(1..1500);
            let teeth = match rng.gen_range(0..5) {
                0 => 1, 1 => 3, 2 => 7, 3 => 25, _ => 90,
            }.min(n);
            let tooth_size = (n / teeth).max(1);

            let mut data: Vec<Keyed> = Vec::with_capacity(n);
            let mut idx = 0u32;
            let mut pos = 0usize;
            for t in 0..teeth {
                let this_len = if t + 1 == teeth { n - pos } else { tooth_size };
                let start = (t * 5) as u32;
                let asc = rng.gen_bool(0.5);
                for k in 0..this_len {
                    let key = if asc { start + k as u32 } else { start + (this_len - 1 - k) as u32 };
                    data.push(Keyed { key, idx });
                    idx += 1;
                }
                pos += this_len;
            }

            let mut reference = data.clone();
            reference.sort();

            let mut v = data.clone();
            let meta = generate_sequential_metadata(&v);
            let offsets = block_offsets(&meta);
            let mut buf = v.clone();
            bottom_up_merge(&mut v, &mut buf, &meta, &offsets, leaf_size, false);

            assert_eq!(
                v.iter().map(|x| (x.key, x.idx)).collect::<Vec<_>>(),
                reference.iter().map(|x| (x.key, x.idx)).collect::<Vec<_>>(),
                "trial {trial}: n={n} teeth={teeth}"
            );
        }
    }

    #[test]
    fn bottom_up_merge_leaf_folds_direction_correctly() {
        let n: u32 = 37;
        let descending: Vec<Keyed> = (0..n).map(|pos| Keyed { key: n - 1 - pos, idx: pos }).collect();
        let metadata = vec![-(n as i64)];
        let offsets = block_offsets(&metadata);

        let mut v1 = descending.clone();
        let mut buf1 = vec![Keyed { key: 0, idx: 0 }; n as usize];
        bottom_up_merge(&mut v1, &mut buf1, &metadata, &offsets, 4, true);
        assert!(is_sorted(&buf1));
        assert!(is_stable(&buf1));

        let mut v2 = descending.clone();
        let mut buf2 = vec![Keyed { key: 0, idx: 0 }; n as usize];
        bottom_up_merge(&mut v2, &mut buf2, &metadata, &offsets, 4, false);
        assert!(is_sorted(&v2));
        assert!(is_stable(&v2));
    }

    #[test]
    fn sorts_already_sorted_data() {
        let mut v: Vec<i32> = (0..50_000).collect();
        multi_merge_sort(&mut v);
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn sorts_reversed_data() {
        let mut v: Vec<i32> = (0..50_000).rev().collect();
        multi_merge_sort(&mut v);
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn stable_on_data_that_used_to_trigger_the_unstable_bailout() {
        let mut rng = StdRng::seed_from_u64(42);
        let n = 50_000usize;
        let mut v: Vec<Keyed> = (0..n as u32)
            .map(|idx| Keyed { key: rng.gen_range(0..25), idx })
            .collect();
        multi_merge_sort(&mut v);
        assert!(is_sorted(&v), "Output is not sorted");
        assert!(is_stable(&v), "Relative order of identical keys was not preserved");
    }

    #[test]
    fn matches_reference_sort_across_random_trials() {
        let mut rng = StdRng::seed_from_u64(7);
        for trial in 0..30 {
            let n: usize = rng.gen_range(0..20_000);
            let mut v: Vec<i64> = (0..n).map(|_| rng.gen_range(-1000..1000)).collect();
            let mut reference = v.clone();
            reference.sort();
            multi_merge_sort(&mut v);
            assert_eq!(v, reference, "Trial {trial} with n={n} diverged from reference sort");
        }
    }

    #[test]
    fn co_rank_boundary_is_valid() {
        let mut rng = StdRng::seed_from_u64(4242);
        for _ in 0..2000 {
            let m: usize = rng.gen_range(0..30);
            let n: usize = rng.gen_range(0..30);
            let mut a: Vec<i32> = (0..m).map(|_| rng.gen_range(0..8)).collect();
            let mut b: Vec<i32> = (0..n).map(|_| rng.gen_range(0..8)).collect();
            a.sort();
            b.sort();
            for k in 0..=(m + n) {
                let (i, j) = co_rank(k, &a, &b);
                assert_eq!(i + j, k);
                assert!(i <= a.len() && j <= b.len());
                if i > 0 && j < b.len() { assert!(a[i - 1] <= b[j]); }
                if j > 0 && i < a.len() { assert!(b[j - 1] < a[i]); }
            }
        }
    }

    #[test]
    fn parallel_merge_hybrid_matches_reference_including_deep_corank_subdivision() {
        let leaf_size = 2;
        let mut rng = StdRng::seed_from_u64(31416);
        for trial in 0..500 {
            let m: usize = rng.gen_range(0..150);
            let n: usize = rng.gen_range(0..150);
            let mut idx = 0u32;
            let mut a: Vec<Keyed> = (0..m).map(|_| { let it = Keyed { key: rng.gen_range(0..10), idx }; idx += 1; it }).collect();
            let mut b: Vec<Keyed> = (0..n).map(|_| { let it = Keyed { key: rng.gen_range(0..10), idx }; idx += 1; it }).collect();
            a.sort();
            b.sort();

            let expected = reference_merge(&a, &b);
            let mut dest = vec![Keyed { key: 0, idx: 0 }; m + n];
            parallel_merge(&mut a, &mut b, &mut dest, leaf_size);

            assert_eq!(
                dest.iter().map(|x| (x.key, x.idx)).collect::<Vec<_>>(),
                expected.iter().map(|x| (x.key, x.idx)).collect::<Vec<_>>(),
                "trial {trial}: m={m} n={n}"
            );
        }
    }

    fn reference_merge(a: &[Keyed], b: &[Keyed]) -> Vec<Keyed> {
        let (mut i, mut j) = (0, 0);
        let mut out = Vec::with_capacity(a.len() + b.len());
        while i < a.len() && j < b.len() {
            if a[i] <= b[j] { out.push(a[i].clone()); i += 1; }
            else { out.push(b[j].clone()); j += 1; }
        }
        out.extend_from_slice(&a[i..]);
        out.extend_from_slice(&b[j..]);
        out
    }
}