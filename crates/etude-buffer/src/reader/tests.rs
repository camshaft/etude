// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use super::*;

/// ensures each implementation returns a trailing chunk correctly
#[test]
#[cfg_attr(miri, ignore)] // This test is too expensive for miri to complete in a reasonable amount of time
fn trailing_chunk_test() {
    let mut dest = vec![];
    let mut source = vec![];
    bolero::check!()
        .with_type::<(u16, u16)>()
        .for_each(|(dest_len, source_len)| {
            source.resize(*source_len as usize, 42);

            let dest_len = (*source_len).min(*dest_len) as usize;
            dest.resize(dest_len, 0);
            let expected = &source[..dest_len];
            let dest = &mut dest[..];

            // direct implementation
            {
                let mut reader: &[u8] = &source[..];
                let mut target = &mut dest[..];

                let chunk = reader.partial_copy_into(&mut target).unwrap();
                assert_eq!(expected, &*chunk);
                assert!(
                    dest.iter().all(|b| *b == 0),
                    "no bytes should be copied into dest"
                );
            }

            // IoSlice implementation
            {
                let io_slice = [&source[..]];
                let mut reader = IoSlice::new(&io_slice);
                let mut target = &mut dest[..];

                let chunk = reader.partial_copy_into(&mut target).unwrap();
                assert_eq!(expected, &*chunk);
                assert!(
                    dest.iter().all(|b| *b == 0),
                    "no bytes should be copied into dest"
                );
            }

            // Buf implementation
            {
                let mut slice = &source[..];
                let mut reader = Buf::new(&mut slice);
                let mut target = &mut dest[..];

                let chunk = reader.partial_copy_into(&mut target).unwrap();
                assert_eq!(expected, &*chunk);
                assert!(
                    dest.iter().all(|b| *b == 0),
                    "no bytes should be copied into dest"
                );
            }

            // full_copy
            {
                let mut source = &source[..];
                let mut reader = source.full_copy();
                let mut target = &mut dest[..];

                let chunk = reader.partial_copy_into(&mut target).unwrap();
                assert!(chunk.is_empty());
                assert_eq!(expected, dest);
                dest.fill(0);
            }
        });
}

/// Chain must yield exactly `a ++ b` in order under any interleaving of the three drain methods
/// (`read_chunk`, `partial_copy_into`, `copy_into`) with fuzz-chosen watermarks and dest
/// capacities, over multi-chunk sources — the a/b seam and the trailing-chunk handling are where a
/// multi-source drain can reorder or drop bytes (breaker-byterope, generalizing the etude#151
/// Builder fence to the combinator this pattern originates from).
#[test]
#[cfg_attr(miri, ignore)] // op-sequence fuzz is too expensive under miri
fn chain_preserves_order_under_mixed_drains() {
    bolero::check!()
        .with_type::<(Vec<Vec<u8>>, Vec<Vec<u8>>, Vec<u8>)>()
        .cloned()
        .for_each(|(a_parts, b_parts, decisions)| {
            let a_slices: alloc::vec::Vec<&[u8]> = a_parts.iter().map(|v| &v[..]).collect();
            let b_slices: alloc::vec::Vec<&[u8]> = b_parts.iter().map(|v| &v[..]).collect();
            let mut model: alloc::vec::Vec<u8> = a_parts.concat();
            model.extend(b_parts.concat());

            let a = IoSlice::new(&a_slices);
            let b = IoSlice::new(&b_slices);
            let mut chain = Chain::new(a, b);
            let mut got: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut scratch = [0u8; 16];

            for d in &decisions {
                let k = usize::from(d >> 2) % (scratch.len() + 1);
                match d % 3 {
                    0 => {
                        let before = chain.buffered_len();
                        let chunk = chain.read_chunk(k).unwrap();
                        assert!(chunk.len() <= k, "read_chunk over watermark");
                        let n = chunk.len();
                        got.extend_from_slice(&chunk);
                        drop(chunk);
                        assert_eq!(
                            chain.buffered_len(),
                            before - n,
                            "read_chunk consumes exactly what it returns"
                        );
                    }
                    1 => {
                        let mut target = &mut scratch[..k];
                        let chunk = chain.partial_copy_into(&mut target).unwrap();
                        let written = k - target.len();
                        assert!(chunk.len() <= target.len(), "trailing chunk fits");
                        let mut piece = scratch[..written].to_vec();
                        piece.extend_from_slice(&chunk);
                        got.extend_from_slice(&piece);
                    }
                    _ => {
                        let mut target = &mut scratch[..k];
                        chain.copy_into(&mut target).unwrap();
                        let written = k - target.len();
                        got.extend_from_slice(&scratch[..written]);
                    }
                }
            }
            // Drain whatever remains, then the stream must be exactly a ++ b.
            loop {
                let chunk = chain.read_chunk(8).unwrap();
                if chunk.is_empty() {
                    break;
                }
                got.extend_from_slice(&chunk);
            }
            assert!(chain.buffer_is_empty());
            assert_eq!(got, model, "chain must yield a ++ b in order");
        });
}
