// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use core::ops::{Deref, DerefMut};

/// Copies vectored slices from one slice into another
///
/// The number of copied items is limited by the minimum of the lengths of each of the slices.
///
/// Returns the number of entries that were copied
#[inline]
pub fn vectored_copy<A, B, T>(from: &[A], to: &mut [B]) -> usize
where
    A: Deref<Target = [T]>,
    B: Deref<Target = [T]> + DerefMut,
    T: Copy,
{
    zip_chunks(from, to, |a, b| {
        b.copy_from_slice(a);
    })
}

/// Zips overlapping chunks from one slice to another
///
/// The number of copied items is limited by the minimum of the lengths of each of the slices.
///
/// Returns the number of entries that were processed
#[inline]
pub fn zip_chunks<A, At, B, Bt, F>(from: &[A], to: &mut [B], mut on_slice: F) -> usize
where
    A: Deref<Target = [At]>,
    B: Deref<Target = [Bt]> + DerefMut,
    F: FnMut(&[At], &mut [Bt]),
{
    let mut count = 0;

    let mut from_index = 0;
    let mut from_offset = 0;

    let mut to_index = 0;
    let mut to_offset = 0;

    // The compiler isn't smart enough to remove all of the bounds checks so we resort to
    // `get_unchecked`.
    //
    // https://godbolt.org/z/45cG1v

    // iterate until we reach one of the ends
    while from_index < from.len() && to_index < to.len() {
        let from = unsafe {
            // Safety: this length is already checked in the while condition
            debug_assert!(from.len() > from_index);
            from.get_unchecked(from_index)
        };

        let to = unsafe {
            // Safety: this length is already checked in the while condition
            debug_assert!(to.len() > to_index);
            to.get_unchecked_mut(to_index)
        };

        {
            // calculate the current views
            let from = unsafe {
                // Safety: the slice offsets are checked at the end of the while loop
                debug_assert!(from.len() >= from_offset);
                from.get_unchecked(from_offset..)
            };

            let to = unsafe {
                // Safety: the slice offsets are checked at the end of the while loop
                debug_assert!(to.len() >= to_offset);
                to.get_unchecked_mut(to_offset..)
            };

            let len = from.len().min(to.len());

            unsafe {
                // Safety: by using the min of the two lengths we will never exceed
                //         either slice's buffer
                debug_assert!(from.len() >= len);
                debug_assert!(to.len() >= len);

                let at = from.get_unchecked(..len);
                let bt = to.get_unchecked_mut(..len);

                on_slice(at, bt);
            }

            // increment the offsets
            from_offset += len;
            to_offset += len;
            count += len;
        }

        // check if the `from` is done
        if from.len() == from_offset {
            from_index += 1;
            from_offset = 0;
        }

        // check if the `to` is done
        if to.len() == to_offset {
            to_index += 1;
            to_offset = 0;
        }
    }

    count
}
