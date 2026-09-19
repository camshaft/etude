// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Unit, oracle, and property tests for the `ByteVec` public surface and its internals.

use super::*;

fn chunk(s: &[u8]) -> Bytes {
    Bytes::copy_from_slice(s)
}

/// A `ByteVec` must stay the footprint of the flat chunk buffer it replaced: the `Deep` variant is
/// boxed, so the value is exactly `len + head Bytes + additional VecDeque`. Pinned to that footprint
/// (not just `<=`) so the tiered representation can never quietly grow past the flat buffer's size.
#[test]
fn size_matches_flat_buffer() {
    let flat_footprint = core::mem::size_of::<usize>()
        + core::mem::size_of::<Bytes>()
        + core::mem::size_of::<VecDeque<Bytes>>();
    assert_eq!(
        core::mem::size_of::<ByteVec>(),
        flat_footprint,
        "ByteVec must stay the flat-buffer footprint (len + head + deque)"
    );
}

/// A `Rope`'s content-kind marker is zero-cost: a `Rope<K>` of ANY kind has the exact layout of
/// `ByteVec` (= `Rope<kind::Bytes>`). This is the layout contract the free `StrRope`/`ByteVec`
/// conversion and the serde-zerocopy path rely on — a future `Rope<Utf8>` must reinterpret to a
/// `ByteVec` with no copy — so it is pinned here against the arbitrary marker below.
#[test]
fn kind_marker_is_zero_cost_layout() {
    struct OtherKind;
    assert_eq!(
        core::mem::size_of::<crate::Rope<OtherKind>>(),
        core::mem::size_of::<ByteVec>(),
        "a content-kind marker must not change the rope's size"
    );
    assert_eq!(
        core::mem::align_of::<crate::Rope<OtherKind>>(),
        core::mem::align_of::<ByteVec>(),
        "a content-kind marker must not change the rope's alignment"
    );
}

/// The `Rope<Utf8>` invariant is over the *concatenated* content: a multi-byte codepoint split
/// across chunk boundaries validates (each chunk alone may be invalid), while genuinely malformed
/// UTF-8 is rejected. This is the operator-ruled invariant (1) — codepoints may span chunks.
#[test]
fn utf8_validation_is_over_concatenation_not_per_chunk() {
    // "é" = 0xC3 0xA9, deliberately split so each chunk alone is NOT valid UTF-8.
    let split: ByteVec = [chunk(&[0xC3]), chunk(&[0xA9])].into_iter().collect();
    assert!(
        core::str::from_utf8(&split.get(0).unwrap()[..]).is_err(),
        "precondition: the first chunk alone is invalid UTF-8"
    );
    let s = Rope::<Utf8>::try_from_bytes(split).expect("é split across chunks is valid UTF-8");
    assert_eq!(&s.copy_to_bytes()[..], "é".as_bytes());

    // Genuinely malformed (0xC3 not followed by a continuation byte) is rejected.
    let bad: ByteVec = [chunk(&[0xC3, 0x28])].into_iter().collect();
    assert!(Rope::<Utf8>::try_from_bytes(bad).is_err());
}

/// `ByteVec` <-> `Rope<Utf8>` round-trips: validated `try_from_bytes` in, free `into_bytes` out,
/// content byte-identical both ways (the free conversion drops only the zero-sized kind marker).
#[test]
fn utf8_bytes_roundtrip_is_lossless() {
    let original: ByteVec = [chunk(b"hello "), chunk("wörld".as_bytes())]
        .into_iter()
        .collect();
    let expected = original.copy_to_bytes();
    let s = Rope::<Utf8>::try_from_bytes(original).expect("valid UTF-8");
    // The kind-agnostic reads are available on Rope<Utf8> (they live on impl<K> Rope<K>).
    assert_eq!(s.len(), expected.len());
    assert!(!s.is_empty());
    assert_eq!(s.byte_at(0), Some(b'h'));
    assert_eq!(s.chunks().count(), 2);
    let back = s.into_bytes();
    assert_eq!(
        back.copy_to_bytes(),
        expected,
        "free round-trip preserves content"
    );
}

/// The caller-trusted `Rope<Utf8>` mutators build content correctly against a `String` model,
/// including `insert_bytes` at the front, an interior boundary, and the end.
#[test]
fn utf8_append_and_insert_bytes_match_string_model() {
    let mut s = Rope::<Utf8>::default();
    s.append_bytes("foo".as_bytes());
    s.append_bytes("bar".as_bytes());
    assert_eq!(&s.copy_to_bytes()[..], b"foobar");

    s.insert_bytes(3, "XYZ".as_bytes()); // foo|bar -> fooXYZbar
    assert_eq!(&s.copy_to_bytes()[..], b"fooXYZbar");
    s.insert_bytes(0, "<".as_bytes()); // front
    assert_eq!(&s.copy_to_bytes()[..], b"<fooXYZbar");
    let end = s.len();
    s.insert_bytes(end, ">".as_bytes()); // end
    assert_eq!(&s.copy_to_bytes()[..], b"<fooXYZbar>");

    // Empty fragments are no-ops.
    s.append_bytes(b"");
    s.insert_bytes(2, b"");
    assert_eq!(&s.copy_to_bytes()[..], b"<fooXYZbar>");
}

/// The streaming validator accepts every valid codepoint no matter WHERE a chunk boundary falls —
/// including inside a 2/3/4-byte codepoint — over every way of cutting the content into up to three
/// chunks, and preserves the content on the accept path.
#[test]
fn utf8_streaming_validates_codepoints_split_at_every_boundary() {
    let samples: &[&str] = &["", "a", "é", "€", "𝄞", "aé€𝄞z", "héllo wörld 𝄞!"];
    for s in samples {
        let full = s.as_bytes();
        for i in 0..=full.len() {
            for j in i..=full.len() {
                let mut rope = ByteVec::new();
                if i > 0 {
                    rope.push_back(chunk(&full[..i]));
                }
                if j > i {
                    rope.push_back(chunk(&full[i..j]));
                }
                if full.len() > j {
                    rope.push_back(chunk(&full[j..]));
                }
                let r = Rope::<Utf8>::try_from_bytes(rope);
                assert!(r.is_ok(), "valid {s:?} split at {i},{j} must validate");
                assert_eq!(
                    &r.unwrap().copy_to_bytes()[..],
                    full,
                    "content preserved {s:?} @ {i},{j}"
                );
            }
        }
    }
}

/// The streaming validator rejects truncated (missing continuation), malformed (lead + non-
/// continuation), and overlong sequences — even when the bad bytes straddle a chunk boundary.
#[test]
fn utf8_streaming_rejects_truncated_and_malformed() {
    // Truncated: drop the final continuation byte of a multi-byte codepoint.
    for s in ["é", "€", "𝄞", "aé", "x€y𝄞"] {
        let full = s.as_bytes();
        let truncated = &full[..full.len() - 1];
        for i in 0..=truncated.len() {
            let mut rope = ByteVec::new();
            if i > 0 {
                rope.push_back(chunk(&truncated[..i]));
            }
            if truncated.len() > i {
                rope.push_back(chunk(&truncated[i..]));
            }
            assert!(
                Rope::<Utf8>::try_from_bytes(rope).is_err(),
                "truncated {s:?} @ {i} must reject"
            );
        }
    }
    // Lead byte followed by a non-continuation byte, plus overlong NUL — at every split.
    let bad: &[&[u8]] = &[
        &[0xC3, 0x28],
        &[0xE2, 0x82, 0x28],
        &[0xF0, 0x9F, 0x28],
        &[0xC0, 0x80],
    ];
    for seq in bad {
        for i in 0..=seq.len() {
            let mut rope = ByteVec::new();
            if i > 0 {
                rope.push_back(chunk(&seq[..i]));
            }
            if seq.len() > i {
                rope.push_back(chunk(&seq[i..]));
            }
            assert!(
                Rope::<Utf8>::try_from_bytes(rope).is_err(),
                "malformed {seq:?} @ {i} must reject"
            );
        }
    }
}

/// Differential: for arbitrary content bytes cut into arbitrary chunk sizes, the streaming
/// `try_from_bytes` agrees EXACTLY with `core::str::from_utf8` over the concatenation (no false
/// accept, no false reject) and preserves the content when it accepts.
#[test]
fn utf8_streaming_matches_from_utf8_oracle() {
    use bolero::check;
    check!()
        .with_type::<(Vec<u8>, Vec<u8>)>()
        .cloned()
        .for_each(|(content, splits)| {
            let sizes: Vec<usize> = if splits.is_empty() {
                vec![1]
            } else {
                splits.iter().map(|b| (*b as usize % 5) + 1).collect()
            };
            let mut rope = ByteVec::new();
            let mut pos = 0;
            let mut si = 0;
            while pos < content.len() {
                let n = sizes[si % sizes.len()].min(content.len() - pos);
                rope.push_back(chunk(&content[pos..pos + n]));
                pos += n;
                si += 1;
            }
            let oracle_ok = core::str::from_utf8(&content).is_ok();
            let r = Rope::<Utf8>::try_from_bytes(rope);
            assert_eq!(r.is_ok(), oracle_ok, "content={content:?} sizes={sizes:?}");
            if let Ok(s) = r {
                assert_eq!(&s.copy_to_bytes()[..], &content[..], "content preserved");
            }
        });
}

/// `as_contiguous` is `Some(borrowed slice)` exactly when the rope is contiguous (empty or one
/// chunk) and `None` once it holds multiple chunks; when `Some`, it equals `copy_to_bytes`.
#[test]
fn as_contiguous_borrows_when_single_chunk() {
    // empty -> Some(&[])
    let empty = ByteVec::new();
    assert_eq!(empty.as_contiguous(), Some(&b""[..]));

    // single chunk -> Some(that slice), borrow equals copy_to_bytes
    let mut one = ByteVec::new();
    one.push_back(chunk(b"hello"));
    assert_eq!(one.as_contiguous(), Some(&b"hello"[..]));
    assert_eq!(one.as_contiguous().unwrap(), &one.copy_to_bytes()[..]);

    // two chunks -> None (not contiguous)
    let mut two = ByteVec::new();
    two.push_back(chunk(b"ab"));
    two.push_back(chunk(b"cd"));
    assert_eq!(two.as_contiguous(), None);

    // deep tier -> None
    let mut deep = ByteVec::new();
    for i in 0..(PROMOTE_AT * 3) {
        deep.push_back(chunk(&[(i % 251) as u8]));
    }
    assert!(matches!(deep.repr, Repr::Deep(_)));
    assert_eq!(deep.as_contiguous(), None);

    // available on Rope<Utf8> too (kind-agnostic), and matches the contiguous content
    let s = Rope::<Utf8>::try_from_bytes(one).unwrap();
    assert_eq!(s.as_contiguous(), Some(&b"hello"[..]));
}

#[test]
fn single_chunk_is_flat_and_allocation_light() {
    let mut rope = ByteVec::new();
    assert!(rope.is_empty());
    rope.push_back(chunk(b"hello"));
    // a single chunk lives in `head`, with no `additional` deque allocated
    match &rope.repr {
        Repr::Small { head, additional } => {
            assert_eq!(&head[..], b"hello");
            assert_eq!(
                additional.capacity(),
                0,
                "single chunk must not allocate a deque"
            );
        }
        _ => panic!("single chunk should stay Small"),
    }
    assert_eq!(rope.len(), 5);
    assert_eq!(rope, b"hello");
}

#[test]
fn empty_chunks_ignored() {
    let mut rope = ByteVec::new();
    rope.push_back(Bytes::new());
    rope.push_front(Bytes::new());
    rope.push_back(chunk(b"a"));
    assert_eq!(rope.len(), 1);
    assert_eq!(rope, b"a");
}

#[test]
fn push_front_back_and_flatten() {
    let mut rope = ByteVec::new();
    rope.push_back(chunk(b"world"));
    rope.push_front(chunk(b"hello "));
    assert_eq!(rope, b"hello world");
    assert_eq!(rope.len(), 11);
}

#[test]
fn pop_front_back() {
    let mut rope: ByteVec = [chunk(b"a"), chunk(b"bb"), chunk(b"ccc")]
        .into_iter()
        .collect();
    assert_eq!(rope.pop_front().unwrap(), &b"a"[..]);
    assert_eq!(rope.pop_back().unwrap(), &b"ccc"[..]);
    assert_eq!(rope.pop_front().unwrap(), &b"bb"[..]);
    assert!(rope.pop_front().is_none());
    assert!(rope.is_empty());
}

#[test]
fn advance_partial_and_whole_chunks() {
    let mut rope: ByteVec = [chunk(b"hello"), chunk(b" "), chunk(b"world")]
        .into_iter()
        .collect();
    rope.advance(3).unwrap(); // partial: inside "hello"
    assert_eq!(rope, b"lo world");
    rope.advance(3).unwrap(); // crosses "lo" + " " into "world"
    assert_eq!(rope, b"world");
    assert!(rope.advance(100).is_err()); // past the end errors, matching the flat buffer
    rope.advance(5).unwrap(); // drain the remaining "world"
    assert!(rope.is_empty());
}

/// Drives the rope past `PROMOTE_AT` (into `Deep`) and back down (into `Small`), checking that
/// order, length, and byte content stay correct across both transitions.
#[test]
fn promotes_and_demotes_preserving_contents() {
    let n = PROMOTE_AT * 4 + 5;
    let mut rope = ByteVec::new();
    let mut expected: VecDeque<Vec<u8>> = VecDeque::new();
    for i in 0..n {
        let b = [(i % 251) as u8, (i % 253) as u8];
        rope.push_back(chunk(&b));
        expected.push_back(b.to_vec());
    }
    assert!(matches!(rope.repr, Repr::Deep(_)), "should have promoted");
    assert_eq!(rope.len(), n * 2);
    let flat: Vec<u8> = expected.iter().flatten().copied().collect();
    assert_eq!(rope, flat);

    // drain from the front; contents stay correct through the demotion boundary
    while let Some(front) = rope.pop_front() {
        let want = expected.pop_front().unwrap();
        assert_eq!(&front[..], &want[..]);
    }
    assert!(rope.is_empty());
    assert!(
        matches!(rope.repr, Repr::Small { .. }),
        "should have demoted"
    );
}

#[test]
fn byte_at_matches_flat_in_both_tiers() {
    for n in [3usize, PROMOTE_AT * 3 + 11] {
        let mut rope = ByteVec::new();
        let mut expected = Vec::new();
        for i in 0..n {
            let b = [(i % 251) as u8, (i % 241) as u8, (i % 239) as u8];
            rope.push_back(chunk(&b));
            expected.extend_from_slice(&b);
        }
        for step in [1usize, 7, 53, 211] {
            let mut idx = 0;
            while idx < expected.len() {
                assert_eq!(rope.byte_at(idx), Some(expected[idx]), "n={n} idx={idx}");
                idx += step;
            }
        }
        assert_eq!(rope.byte_at(expected.len()), None);
        // `chunks().len()` is the chunk-count accessor (parity with ByteVec); it must be exact.
        assert_eq!(rope.chunks().len(), rope.chunks().count(), "n={n}");
    }
}

/// `starts_with` / `ends_with` against a `Vec<u8>` oracle, in both tiers, with the literal spanning
/// chunk boundaries. Covers empty literals, exact-length, and longer-than-buffer.
/// The `FromIterator` bulk path (exact `size_hint` above `PROMOTE_AT` routes into the bottom-up
/// tree build) must survive hostile-but-safe inputs: empty chunks (the `extend_blocks` filter is
/// load-bearing — without it the no-empty-chunks invariant corrupts), and a LYING `size_hint`
/// (correctness must follow the actually-yielded items, with the tree/demote path normalizing tiny
/// or empty results back to a valid rope).
#[test]
fn from_iter_survives_empty_chunks_and_lying_size_hints() {
    // Bulk collect of ONLY empty chunks (exact hint 100 > PROMOTE_AT): a valid empty rope.
    let rope: ByteVec = vec![Bytes::new(); 100].into_iter().collect();
    rope.check_invariants();
    assert!(rope.is_empty());
    assert_eq!(rope.chunks().len(), 0);
    assert_eq!(rope, ByteVec::new());

    // An iterator whose size_hint lower bound lies high: routing must not affect correctness.
    struct Liar(alloc::vec::IntoIter<Bytes>);
    impl Iterator for Liar {
        type Item = Bytes;
        fn next(&mut self) -> Option<Bytes> {
            self.0.next()
        }
        fn size_hint(&self) -> (usize, Option<usize>) {
            (1000, None)
        }
    }
    let rope: ByteVec =
        Liar(vec![Bytes::from_static(b"ab"), Bytes::from_static(b"cd")].into_iter()).collect();
    rope.check_invariants();
    assert_eq!(rope, b"abcd"[..]);
    assert_eq!(rope.chunks().len(), 2);

    let rope: ByteVec = Liar(vec![].into_iter()).collect();
    rope.check_invariants();
    assert!(rope.is_empty());
    assert_eq!(rope, ByteVec::new());

    // Bulk build with interleaved empties: content, chunk count, and flatten follow the
    // non-empty chunks only.
    let chunks: Vec<Bytes> = (0..100u8)
        .map(|i| {
            if i % 3 == 0 {
                Bytes::new()
            } else {
                Bytes::copy_from_slice(&[i])
            }
        })
        .collect();
    let want: Vec<u8> = (0..100u8).filter(|i| i % 3 != 0).collect();
    let rope: ByteVec = chunks.into_iter().collect();
    rope.check_invariants();
    assert_eq!(rope, want[..]);
    assert_eq!(rope.chunks().len(), want.len());
    let flat: Vec<u8> = rope.chunks().flat_map(|c| c.iter().copied()).collect();
    assert_eq!(flat, want);
}

#[test]
fn starts_ends_with_match_flat_in_both_tiers() {
    for n in [4usize, PROMOTE_AT * 3 + 7] {
        let mut rope = ByteVec::new();
        let mut model: Vec<u8> = Vec::new();
        for i in 0..n {
            let b = [(i % 251) as u8, (i % 241) as u8, (i % 239) as u8];
            rope.push_back(chunk(&b));
            model.extend_from_slice(&b);
        }
        let total = model.len();

        // Prefixes/suffixes of several lengths, incl. spanning chunk boundaries, plus edge cases.
        let lens = [
            0usize,
            1,
            2,
            3,
            5,
            8,
            total / 2,
            total - 1,
            total,
            total + 1,
        ];
        for &k in &lens {
            let want_prefix = k <= total;
            let pref: Vec<u8> = model.iter().take(k).copied().collect();
            // Only meaningful when k <= total; when k > total, `pref` is the whole buffer (< k), so
            // starts_with must be false — build an over-long literal explicitly for that case.
            if want_prefix {
                assert!(rope.starts_with(&pref), "n={n} starts_with len={k}");
                let suf: Vec<u8> = model.iter().rev().take(k).rev().copied().collect();
                assert!(rope.ends_with(&suf), "n={n} ends_with len={k}");
            } else {
                // A literal longer than the whole buffer never matches.
                let mut over = model.clone();
                over.push(0xAB);
                assert!(!rope.starts_with(&over), "n={n} over-long starts_with");
                assert!(!rope.ends_with(&over), "n={n} over-long ends_with");
            }
        }

        // Negatives: flip the last/first byte of an otherwise-matching literal.
        if total >= 2 {
            let mut bad_pref = model[..3.min(total)].to_vec();
            *bad_pref.last_mut().unwrap() ^= 0xFF;
            assert!(!rope.starts_with(&bad_pref), "n={n} mismatched prefix");
            let mut bad_suf = model[total - 3.min(total)..].to_vec();
            bad_suf[0] ^= 0xFF;
            assert!(!rope.ends_with(&bad_suf), "n={n} mismatched suffix");
        }

        // Empty literal always matches; whole-buffer literal matches both ends.
        assert!(rope.starts_with(b""));
        assert!(rope.ends_with(b""));
        assert!(rope.starts_with(&model));
        assert!(rope.ends_with(&model));
    }
}

/// The chunk iterator is a `DoubleEndedIterator`: `.rev()` yields the forward chunks in reverse in
/// both tiers, `ExactSizeIterator::len` stays exact, and — the property the two-cursor design has to
/// get right — an interleaved front/back walk yields every chunk exactly once with no overlap.
#[test]
fn chunks_is_double_ended_in_both_tiers() {
    for n in [1usize, 4, PROMOTE_AT * 3 + 7] {
        let mut rope = ByteVec::new();
        let mut fwd: Vec<Vec<u8>> = Vec::new();
        for i in 0..n {
            let b = [(i % 251) as u8, (i % 241) as u8];
            rope.push_back(chunk(&b));
            fwd.push(b.to_vec());
        }
        // `.rev()` == forward chunks reversed
        let rev: Vec<Vec<u8>> = rope.chunks().rev().map(|c| c.to_vec()).collect();
        let mut fwd_rev = fwd.clone();
        fwd_rev.reverse();
        assert_eq!(rev, fwd_rev, "n={n}");
        // ExactSizeIterator::len is exact
        assert_eq!(
            rope.chunks().rev().len(),
            rope.chunks().count(),
            "n={n} len"
        );
        // Re-reversing the reverse walk reconstructs the original byte content.
        let flat: Vec<u8> = fwd.iter().flatten().copied().collect();
        let mut round: Vec<u8> = Vec::new();
        for c in rope.chunks().rev() {
            round.splice(0..0, c.iter().copied());
        }
        assert_eq!(round, flat, "n={n} roundtrip");

        // Interleave next/next_back: alternately take from the front and the back. The collected
        // front-prefix ++ reversed(back-suffix) must reconstruct the exact forward chunk sequence,
        // proving the two cursors partition the chunks (no chunk yielded twice, none skipped).
        let mut it = rope.chunks();
        let mut front: Vec<*const u8> = Vec::new();
        let mut back: Vec<*const u8> = Vec::new();
        let mut take_front = true;
        loop {
            let got = if take_front {
                it.next()
            } else {
                it.next_back()
            };
            match got {
                Some(c) if take_front => front.push(c.as_ptr()),
                Some(c) => back.push(c.as_ptr()),
                None => break,
            }
            take_front = !take_front;
        }
        let mut seq = front;
        seq.extend(back.into_iter().rev());
        let want: Vec<*const u8> = rope.chunks().map(|c| c.as_ptr()).collect();
        assert_eq!(seq, want, "n={n} interleaved front/back partition");
        assert_eq!(seq.len(), n, "n={n} interleaved yielded every chunk once");
    }
}

#[test]
fn set_byte_in_both_tiers_and_cow_preserves_shared() {
    for n in [3usize, PROMOTE_AT * 3 + 11] {
        let mut rope = ByteVec::new();
        let mut expected = Vec::new();
        for i in 0..n {
            let b = [(i % 251) as u8, (i % 241) as u8, (i % 239) as u8];
            rope.push_back(chunk(&b));
            expected.extend_from_slice(&b);
        }
        // A clone shares every chunk (and, in the deep tier, the tree spine); writes to `rope`
        // must copy-on-write and leave the snapshot untouched.
        let snapshot = rope.clone();
        let snap_bytes = expected.clone();
        for step in [1usize, 7, 53, 211] {
            let mut idx = step % expected.len();
            while idx < expected.len() {
                let v = (idx as u8).wrapping_mul(7).wrapping_add(step as u8);
                rope.set_byte(idx, v).unwrap();
                expected[idx] = v;
                idx += 97;
            }
        }
        assert_eq!(rope, expected, "n={n}");
        assert_eq!(
            snapshot, snap_bytes,
            "shared snapshot observed a write, n={n}"
        );
        assert_eq!(
            rope.set_byte(expected.len(), 0),
            Err(ByteVecError::OutOfBounds(expected.len())),
        );
    }
}

#[test]
fn replace_in_both_tiers_and_cow_preserves_shared() {
    for n in [4usize, PROMOTE_AT * 3 + 7] {
        let mut rope = ByteVec::new();
        let mut model = Vec::new();
        for i in 0..n {
            let b = [
                (i % 251) as u8,
                (i % 241) as u8,
                (i % 239) as u8,
                (i % 233) as u8,
            ];
            rope.push_back(chunk(&b));
            model.extend_from_slice(&b);
        }
        // A clone shares chunks/spine; every edit below must copy-on-write and leave it intact.
        let snapshot = rope.clone();
        let snap = model.clone();

        // Replace a middle range with an owned Bytes.
        let (a, b) = (model.len() / 4, model.len() / 2);
        rope.replace(a..b, Bytes::from_static(b"HELLO")).unwrap();
        model.splice(a..b, b"HELLO".iter().copied());
        assert_eq!(rope, model, "replace-Bytes n={n}");

        // Insert (empty range) with a borrowed slice — single byte is the same shape.
        rope.replace(a..a, &b"+"[..]).unwrap();
        model.splice(a..a, b"+".iter().copied());
        assert_eq!(rope, model, "insert-slice n={n}");

        // Delete (empty value).
        rope.replace(0..3, &b""[..]).unwrap();
        model.splice(0..3, core::iter::empty());
        assert_eq!(rope, model, "delete n={n}");

        // Replace the tail with another ByteVec (zero-copy chunk splice).
        let vr: ByteVec = [chunk(b"aa"), chunk(b"bbb")].into_iter().collect();
        let e = model.len();
        rope.replace(e..e, vr).unwrap();
        model.extend_from_slice(b"aabbb");
        assert_eq!(rope, model, "append-rope n={n}");

        assert_eq!(snapshot, snap, "shared snapshot observed a write, n={n}");
        let l = rope.len();
        assert!(matches!(
            rope.replace(l + 1..l + 1, &b"x"[..]),
            Err(ByteVecError::OutOfBounds(_))
        ));
    }
}

/// RED reproducer (breaker-bytevec): the range-bound resolution in `replace` (`resolve_range`)
/// and `slice` computes `Included(e) => e + 1` / `Excluded(s) => s + 1` UNCHECKED. In release
/// builds the add wraps: `replace(0..=usize::MAX, v)` resolves to `(0, 0)` and silently INSERTS
/// at the front returning `Ok` (observed: b"Xhello world") instead of the documented
/// `Err(OutOfBounds)`; `slice((Excluded(usize::MAX), Unbounded))` silently returns the whole
/// rope instead of the documented panic. In debug builds both die with an arithmetic-overflow
/// panic, which for `replace` also violates the documented `Err` contract. Fix shape:
/// `checked_add(1)` — `None` maps to `Err(OutOfBounds(usize::MAX))` in `resolve_range` and to
/// the documented out-of-bounds panic in `slice`. This test asserts the contract and FAILS in
/// BOTH build modes today (debug: overflow panic; release: Ok + mutation).
#[test]
fn replace_with_inclusive_max_end_errors_instead_of_wrapping() {
    let mut rope = ByteVec::from(b"hello world");
    assert_eq!(
        rope.replace(0..=usize::MAX, &b"X"[..]),
        Err(ByteVecError::OutOfBounds(usize::MAX)),
        "an out-of-bounds inclusive end must error, not wrap"
    );
    assert_eq!(
        rope, b"hello world",
        "the failed replace must not mutate the rope"
    );
    // excluded start overflow takes the same unchecked path in resolve_range
    assert_eq!(
        rope.replace(
            (
                core::ops::Bound::Excluded(usize::MAX),
                core::ops::Bound::Unbounded
            ),
            &b"X"[..]
        ),
        Err(ByteVecError::OutOfBounds(usize::MAX)),
        "an out-of-bounds excluded start must error, not wrap"
    );
    assert_eq!(rope, b"hello world");
}

/// Companion to `replace_with_inclusive_max_end_errors_instead_of_wrapping` for the `slice` path:
/// an excluded start of `usize::MAX` must hit `slice`'s documented out-of-bounds panic, not wrap
/// the `+ 1` to 0 and silently return the whole rope (release) or arithmetic-overflow (debug).
#[test]
#[should_panic(expected = "out of bounds")]
fn slice_with_excluded_max_start_panics_instead_of_wrapping() {
    let rope = ByteVec::from(b"hello world");
    let _ = rope.slice((
        core::ops::Bound::Excluded(usize::MAX),
        core::ops::Bound::Unbounded,
    ));
}

#[test]
fn replace_equal_length_overwrite_is_in_place() {
    // A unique single-chunk rope overwritten with equal-length values must stay ONE chunk (the
    // Path-1 in-place write — no allocation, no fragmentation).
    let mut rope: ByteVec = Bytes::from(vec![0u8; 1000]).into();
    let mut model = vec![0u8; 1000];
    for i in 0..300usize {
        let at = (i * 7) % 997;
        rope.replace(at..at + 3, &b"abc"[..]).unwrap();
        model.splice(at..at + 3, b"abc".iter().copied());
    }
    assert_eq!(rope, model);
    assert_eq!(
        rope.chunks().len(),
        1,
        "equal-length overwrite fragmented the chunk"
    );
}

#[test]
fn replace_equal_length_overwrite_spans_chunks_in_place() {
    let mut rope: ByteVec = [
        chunk(b"aaaa"),
        chunk(b"bbbb"),
        chunk(b"cccc"),
        chunk(b"dddd"),
    ]
    .into_iter()
    .collect();
    let mut model = b"aaaabbbbccccdddd".to_vec();
    let before = rope.chunks().len();
    // [2, 10) spans chunks 0,1,2 with an equal-length (8-byte) value — must stay in place.
    rope.replace(2..10, &b"XYZWVUTS"[..]).unwrap();
    model.splice(2..10, b"XYZWVUTS".iter().copied());
    assert_eq!(rope, model);
    assert_eq!(
        rope.chunks().len(),
        before,
        "spanning overwrite changed chunk count"
    );

    // Copy-on-write: a shared snapshot must not observe the overwrite.
    let snap: ByteVec = [
        chunk(b"aaaa"),
        chunk(b"bbbb"),
        chunk(b"cccc"),
        chunk(b"dddd"),
    ]
    .into_iter()
    .collect();
    let shared = snap.clone();
    let mut editable = snap;
    editable.replace(0..8, &b"01234567"[..]).unwrap();
    assert_eq!(editable, b"01234567ccccdddd");
    assert_eq!(
        shared, b"aaaabbbbccccdddd",
        "snapshot observed the overwrite"
    );
}

#[test]
fn uc5_brute_force() {
    for nchunks in [6usize, 12, PROMOTE_AT * 2] {
        let build = || {
            let mut r = ByteVec::new();
            let mut m = Vec::new();
            for i in 0..nchunks {
                let b = [(i * 3) as u8, (i * 3 + 1) as u8, (i * 3 + 2) as u8];
                r.push_back(chunk(&b));
                m.extend_from_slice(&b);
            }
            (r, m)
        };
        let total = nchunks * 3;
        let step = if total > 60 { 7 } else { 1 };
        for start in (0..=total).step_by(step) {
            for end in (start..=total).step_by(step) {
                for vlen in [0usize, 1, 5, 40] {
                    for kind in 0..3 {
                        let (mut r, mut m) = build();
                        let val: Vec<u8> = (0..vlen).map(|k| 200u8.wrapping_add(k as u8)).collect();
                        match kind {
                            0 => r.replace(start..end, &val[..]).unwrap(),
                            1 => r.replace(start..end, Bytes::from(val.clone())).unwrap(),
                            _ => {
                                let vr: ByteVec =
                                    val.chunks(3).map(Bytes::copy_from_slice).collect();
                                r.replace(start..end, vr).unwrap();
                            }
                        }
                        m.splice(start..end, val.iter().copied());
                        assert_eq!(r, m, "n={nchunks} [{start}..{end}) vlen={vlen} kind={kind}");
                    }
                }
            }
        }
    }
}

#[test]
fn replace_small_inserts_coalesce_and_do_not_fragment() {
    // Many single-byte inserts within a chunk must collapse (UC4), not leave ~1-byte chunks.
    let mut rope = ByteVec::new();
    let mut model: Vec<u8> = Vec::new();
    for _ in 0..400usize {
        let at = model.len() / 2;
        rope.replace(at..at, &b"x"[..]).unwrap();
        model.splice(at..at, core::iter::once(b'x'));
    }
    assert_eq!(rope, model);
    let chunk_count = rope.chunks().len();
    assert!(
        chunk_count < 40,
        "coalescing failed: {chunk_count} chunks for 400 bytes"
    );
}

#[test]
fn replace_structural_deep_tier() {
    let n = PROMOTE_AT * 3 + 5;
    let mut rope = ByteVec::new();
    let mut model: Vec<u8> = Vec::new();
    for i in 0..n {
        let b = [(i % 251) as u8, (i % 241) as u8, (i % 239) as u8];
        rope.push_back(chunk(&b));
        model.extend_from_slice(&b);
    }
    // Populate the head buffer so splices span head + tree + tail.
    for i in 0..(FANOUT + 3) {
        let b = [(140 + (i % 60)) as u8, (i % 37) as u8];
        rope.push_front(chunk(&b));
        model.splice(0..0, b.iter().copied());
    }
    let snapshot = rope.clone();
    let snap = model.clone();

    // Length-changing structural edits (insert / delete / replace) at varied offsets and sizes.
    for &(at, del, ins) in &[
        (0usize, 0usize, 5usize), // insert at front
        (10, 40, 3),              // shrink, spanning chunks
        (200, 5, 90),             // grow, spanning chunks
        (0, 30, 0),               // delete at front
    ] {
        let at = at.min(model.len());
        let del = del.min(model.len() - at);
        let val: Vec<u8> = (0..ins).map(|k| ((at + k) as u8) ^ 0x71).collect();
        rope.replace(at..at + del, Bytes::from(val.clone()))
            .unwrap();
        model.splice(at..at + del, val.iter().copied());
        assert_eq!(rope, model, "deep structural at={at} del={del} ins={ins}");
    }
    // delete to the very end, and a borrowed-slice value
    let l = model.len();
    rope.replace(l - 7..l, &b"tail!"[..]).unwrap();
    model.splice(l - 7..l, b"tail!".iter().copied());
    assert_eq!(rope, model, "deep structural to-end");

    assert_eq!(snapshot, snap, "shared snapshot observed a structural edit");
}

#[test]
fn replace_equal_length_overwrite_deep_tier() {
    let n = PROMOTE_AT * 3 + 5;
    let mut rope = ByteVec::new();
    let mut model: Vec<u8> = Vec::new();
    for i in 0..n {
        let b = [(i % 251) as u8, (i % 241) as u8, (i % 239) as u8];
        rope.push_back(chunk(&b));
        model.extend_from_slice(&b);
    }
    // Populate the head buffer too, so overwrites exercise head + tree + tail.
    for i in 0..(FANOUT + 3) {
        let b = [(140 + (i % 60)) as u8, (i % 37) as u8];
        rope.push_front(chunk(&b));
        model.splice(0..0, b.iter().copied());
    }
    let chunks_before = rope.chunks().len();
    let snapshot = rope.clone();
    let snap = model.clone();

    let total = model.len();
    for &(at, len) in &[
        (0usize, 10usize),
        (total / 3, 100),
        (total / 2, 120),
        (total - 20, 20),
    ] {
        let val: Vec<u8> = (0..len).map(|k| ((at + k) as u8) ^ 0x33).collect();
        rope.replace(at..at + len, Bytes::from(val.clone()))
            .unwrap();
        model.splice(at..at + len, val.iter().copied());
        assert_eq!(rope, model, "deep overwrite at {at} len {len}");
    }
    // A borrowed-slice value (no allocation path).
    rope.replace(50..60, &b"0123456789"[..]).unwrap();
    model.splice(50..60, b"0123456789".iter().copied());
    assert_eq!(rope, model, "deep overwrite with slice value");

    // Equal-length overwrite never changes structure.
    assert_eq!(
        rope.chunks().len(),
        chunks_before,
        "deep overwrite changed chunk count"
    );
    assert_eq!(snapshot, snap, "shared snapshot observed a write");
}

/// Exercises split_to / split_to_copy / truncate / append / get / copy_to_bytes in both tiers,
/// checking results against a flat `Vec<u8>` oracle.
#[test]
fn public_api_matches_oracle_in_both_tiers() {
    for n in [5usize, PROMOTE_AT * 3 + 9] {
        let make = || -> (ByteVec, Vec<u8>) {
            let mut rope = ByteVec::new();
            let mut flat = Vec::new();
            for i in 0..n {
                let b = [(i % 251) as u8, (i % 241) as u8, (i % 239) as u8];
                rope.push_back(chunk(&b));
                flat.extend_from_slice(&b);
            }
            (rope, flat)
        };

        // split_to
        let (mut rope, flat) = make();
        let at = flat.len() / 3;
        let front = rope.split_to(at).unwrap();
        assert_eq!(front, &flat[..at]);
        assert_eq!(rope, &flat[at..]);
        assert!(rope.split_to(rope.len() + 1).is_err());

        // split_to_copy
        let (mut rope, flat) = make();
        let copied = rope.split_to_copy(at).unwrap();
        assert_eq!(&copied[..], &flat[..at]);
        assert_eq!(rope, &flat[at..]);

        // truncate
        let (mut rope, flat) = make();
        let keep = flat.len() / 2;
        rope.truncate(keep);
        assert_eq!(rope, &flat[..keep]);

        // append
        let (mut a, fa) = make();
        let (mut b, fb) = make();
        a.append(&mut b);
        assert!(b.is_empty());
        let mut expected = fa.clone();
        expected.extend_from_slice(&fb);
        assert_eq!(a, expected);

        // copy_to_bytes
        let (rope, flat) = make();
        assert_eq!(&rope.copy_to_bytes()[..], &flat[..]);

        // get (chunk index): first chunk is bytes 0..3
        let (rope, _) = make();
        assert_eq!(rope.get(0).unwrap().len(), 3);
        assert_eq!(rope.get(n), None);
    }
}

fn deep_rope(n: usize) -> (ByteVec, Vec<u8>) {
    let mut rope = ByteVec::new();
    let mut flat = Vec::new();
    for i in 0..n {
        // varied chunk sizes so the tree is genuinely relaxed
        let clen = 1 + (i % 5);
        let b: Vec<u8> = (0..clen).map(|k| (i + k) as u8).collect();
        rope.push_back(Bytes::from(b.clone()));
        flat.extend_from_slice(&b);
    }
    (rope, flat)
}

/// `concat` (via `append`) across many size pairs — including height mismatches — must equal the
/// flat concatenation, with correct len/chunk bytes throughout.
#[test]
fn concat_matches_oracle() {
    for &na in &[0usize, 1, 5, 40, 200, 1000] {
        for &nb in &[0usize, 1, 5, 40, 200, 1000] {
            let (mut a, fa) = deep_rope(na);
            let (mut b, fb) = deep_rope(nb);
            a.append(&mut b);
            assert!(b.is_empty(), "other emptied ({na},{nb})");
            let mut expected = fa.clone();
            expected.extend_from_slice(&fb);
            assert_eq!(a.len(), expected.len(), "len ({na},{nb})");
            assert_eq!(a, expected, "bytes ({na},{nb})");
            // spot-check random-access agrees end to end
            if !expected.is_empty() {
                for off in [0, expected.len() / 2, expected.len() - 1] {
                    assert_eq!(a.byte_at(off), Some(expected[off]), "get {off} ({na},{nb})");
                }
            }
        }
    }
}

/// `split_to` at many offsets (both tiers) must produce the two exact halves, each internally
/// consistent (len, bytes, random access).
#[test]
fn split_matches_oracle() {
    for &n in &[1usize, 5, 40, 200, 1000] {
        let (_, flat) = deep_rope(n);
        let total = flat.len();
        for at in [0, 1, total / 3, total / 2, total.saturating_sub(1), total] {
            if at > total {
                continue;
            }
            let (mut rope, _) = deep_rope(n);
            let front = rope.split_to(at).unwrap();
            assert_eq!(front.len(), at, "front len n={n} at={at}");
            assert_eq!(front, &flat[..at], "front bytes n={n} at={at}");
            assert_eq!(rope.len(), total - at, "back len n={n} at={at}");
            assert_eq!(rope, &flat[at..], "back bytes n={n} at={at}");
            // random access on both halves
            if at > 0 {
                assert_eq!(front.byte_at(at - 1), Some(flat[at - 1]));
            }
            if at < total {
                assert_eq!(rope.byte_at(0), Some(flat[at]));
            }
        }
    }
}

/// A split immediately re-concatenated must reproduce the original, for offsets across a deep
/// rope (exercises subtree sharing on both operations).
#[test]
fn slice_matches_oracle_in_both_tiers() {
    for &n in &[4usize, 1000] {
        let (rope, flat) = deep_rope(n);
        let total = flat.len();
        for (a, b) in [
            (0, total),
            (0, total / 2),
            (total / 4, total * 3 / 4),
            (total / 2, total),
            (total, total),
            (7, 7),
            (1, total - 1),
        ] {
            let s = rope.slice(a..b);
            assert_eq!(s.len(), b - a, "n={n} {a}..{b}");
            assert_eq!(s, &flat[a..b], "n={n} {a}..{b}");
        }
        // open-ended forms
        assert_eq!(rope.slice(..), flat);
        assert_eq!(rope.slice(10..), &flat[10..]);
        assert_eq!(rope.slice(..total - 3), &flat[..total - 3]);
    }
}

/// A deep rope with bytes in ALL THREE regions — buffered head, tree, buffered tail — so a slice
/// can straddle the head→tree and tree→tail seams (`deep_rope` alone leaves the head empty).
fn deep_rope_with_buffered_ends() -> (ByteVec, Vec<u8>) {
    let (mut rope, mut flat) = deep_rope(1000);
    for i in 0..9u8 {
        let b = alloc::vec![200 + i; 2 + i as usize];
        rope.push_front(Bytes::from(b.clone())); // populate the buffered head
        let mut prefixed = b;
        prefixed.extend_from_slice(&flat);
        flat = prefixed;
    }
    assert!(matches!(rope.repr, Repr::Deep(_)));
    (rope, flat)
}

/// `slice` must match the flat oracle for EVERY range, including ones that straddle the buffered
/// head/tail seams — the region-walking reattach path in the deep tier.
#[test]
fn slice_across_buffered_head_tree_tail() {
    let (rope, flat) = deep_rope_with_buffered_ends();
    let total = flat.len();
    // coarse grid over all three regions + the near-boundary bytes at each end
    let mut points: Vec<usize> = (0..=6).collect();
    points.extend([total / 4, total / 2, total * 3 / 4]);
    points.extend(total - 6..=total);
    for &a in &points {
        for &b in &points {
            if a <= b {
                let s = rope.slice(a..b);
                assert_eq!(s.len(), b - a, "{a}..{b}");
                assert_eq!(s, &flat[a..b], "{a}..{b}");
            }
        }
    }
}

/// Appending a rope to a CLONE of itself must produce a shared DAG (the same subtree referenced
/// twice), never a cyclic/exploding structure: length doubles, content is the original twice, the
/// original is untouched, and a later in-place edit copies-on-write instead of aliasing.
#[test]
fn self_append_shares_structure_without_exploding() {
    let (mut a, flat) = deep_rope(1000);
    let mut b = a.clone(); // b aliases every one of a's subtrees (rc >= 2)
    a.append(&mut b);

    // Correct doubling, no infinite recursion.
    assert_eq!(a.len(), 2 * flat.len());
    let mut doubled = flat.clone();
    doubled.extend_from_slice(&flat);
    assert_eq!(a, doubled);
    a.check_invariants();

    // The clone we appended was drained; a fresh clone of the original is still intact.
    let (orig, _) = deep_rope(1000);
    assert_eq!(orig, flat);

    // COW on the shared DAG: editing the doubled rope must not corrupt an independent clone.
    let snapshot = a.clone();
    a.set_byte(0, 0xAB).unwrap();
    a.set_byte(flat.len(), 0xCD).unwrap(); // the seam byte, in the second (shared) copy
    assert_eq!(snapshot, doubled, "COW: shared snapshot unchanged");
    assert_eq!(a.byte_at(0), Some(0xAB));
    assert_eq!(a.byte_at(flat.len()), Some(0xCD));
}

/// Repeatedly appending a rope to a clone of itself must NOT explode. Each append shares the whole
/// prior structure (O(log) new spine nodes + Arc bumps), so 20 doublings — a logical length of
/// millions — stays cheap in memory and completes instantly. We only touch O(1) length and O(log)
/// random access; a full traversal would be O(logical length) precisely because the data is real.
#[test]
fn repeated_self_append_does_not_explode() {
    let (mut r, _) = deep_rope(64);
    let base = r.len();
    r.check_invariants(); // structure is valid before we start sharing it
    for i in 1..=20 {
        let mut clone = r.clone(); // O(1): bumps the root Arc
        r.append(&mut clone);
        assert_eq!(r.len(), base << i, "length must double exactly at step {i}");
    }
    // Random access still resolves in O(log) against the shared DAG.
    assert_eq!(r.byte_at(0), Some(0));
    assert_eq!(r.byte_at(r.len() - 1), r.byte_at(base - 1));
}

#[test]
fn split_then_concat_roundtrips() {
    let (_, flat) = deep_rope(777);
    for at in [0, 3, 100, 388, 776, 777] {
        let (mut rope, _) = deep_rope(777);
        let mut front = rope.split_to(at).unwrap();
        front.append(&mut rope);
        assert_eq!(front, flat, "roundtrip at={at}");
        assert_eq!(front.len(), flat.len());
    }
}

#[test]
fn trait_impls_behave() {
    use etude_buffer::{reader::Buffer as _, writer::Buffer as _};

    // writer::Buffer + PartialEq + Debug
    let mut w = ByteVec::new();
    w.put_slice(b"hello ");
    w.put_bytes(Bytes::from_static(b"world"));
    assert_eq!(w, b"hello world");
    assert_eq!(w, "hello world");
    assert_eq!(&format!("{w:?}"), "[b\"hello \", b\"world\"]");

    // Index (chunk-granular) + get
    assert_eq!(&w[0][..], b"hello ");
    assert_eq!(&w[1][..], b"world");

    // bytes::Buf: chunk() / advance() / copy_to_bytes() (fully-qualified — inherent methods of the
    // same name shadow the trait ones for method-call syntax, exactly as on ByteVec)
    let mut b = w.clone();
    assert_eq!(bytes::Buf::remaining(&b), 11);
    assert_eq!(bytes::Buf::chunk(&b), b"hello ");
    bytes::Buf::advance(&mut b, 6);
    assert_eq!(bytes::Buf::copy_to_bytes(&mut b, 5), &b"world"[..]);
    assert_eq!(bytes::Buf::remaining(&b), 0);

    // reader::Buffer into a BytesMut destination (non-consuming reader over a clone)
    let mut r = w.reader();
    let mut dst = bytes::BytesMut::new();
    r.copy_into(&mut dst).unwrap();
    assert_eq!(&dst[..], b"hello world");
    assert_eq!(w.len(), 11, "reader() must not consume the original");

    // IntoIterator drains chunks
    let chunks: Vec<Bytes> = w.clone().into_iter().collect();
    assert_eq!(chunks.len(), 2);

    // From/Into round-trips
    assert_eq!(ByteVec::from(&b"abc"[..]), b"abc");
    assert_eq!(ByteVec::from(vec![1u8, 2, 3]), [1u8, 2, 3]);
    let v: Vec<Bytes> =
        ByteVec::from_iter([Bytes::from_static(b"x"), Bytes::from_static(b"y")]).into();
    assert_eq!(v.len(), 2);
}

#[cfg(feature = "std")]
#[test]
fn io_read_write() {
    use std::io::{Read as _, Write as _};
    let mut rope = ByteVec::new();
    rope.write_all(b"hello ").unwrap();
    rope.write_all(b"world").unwrap();
    assert_eq!(rope, b"hello world");

    let mut out = [0u8; 5];
    let n = rope.read(&mut out).unwrap();
    assert_eq!(&out[..n], b"hello");
    assert_eq!(rope, b" world");
}

/// Differential fuzz — THE shared harness for every byterope agent (breaker/fixer/compat):
/// extend the `Op` enum here rather than adding one-off differential loops. Applies a random op
/// sequence to a `ByteVec` and to a flat `Vec<u8>` model, asserting byte-for-byte equivalence
/// (and per-op results) after every step. Covers the full public surface, including:
/// - deep-tier starts in one op (`Deepen` populates tree + buffered ends, so sequences reach the
///   deep paths without needing 64+ pushes),
/// - zero-copy aliasing in BOTH directions: `TakeAliases` retains a full clone and a zero-copy
///   slice that every subsequent op must leave untouched (asserted after each op), and
///   `MutateAlias` writes THROUGH the slice while the parent must stay intact,
/// - a retained shared chunk (`PushShared`) that no COW edit may ever write through (asserted at
///   the end of every run),
/// - self-sharing appends, `bytes::Buf` reads, `split_to_copy`, and flatten/chunks roundtrips.
#[test]
fn differential_against_model() {
    use bolero::check;
    use bolero_generator::TypeGenerator;

    #[derive(Debug, Clone, TypeGenerator)]
    enum Op {
        PushBack(Vec<u8>),
        PushFront(Vec<u8>),
        PopFront,
        PopBack,
        Advance(usize),
        Truncate(usize),
        SplitTo(usize),
        SplitToCopy(usize),
        Append(Vec<u8>),
        AppendSelfSlice(usize, usize),
        Slice(usize, usize),
        GetByte(usize),
        GetChunk(usize),
        SetByte(usize, u8),
        Replace(usize, usize, Vec<u8>, u8),
        BufRead(usize, usize),
        FlattenCheck,
        Deepen,
        Clear,
        TakeAliases(usize, usize),
        MutateAlias(usize, u8),
        PushShared,
        StartsEndsWith(usize, Vec<u8>),
        DoubleEndedCheck,
        InterleavedChunksCheck(Vec<bool>),
        AsContiguousCheck,
    }

    check!().with_type::<Vec<Op>>().cloned().for_each(|ops| {
        let mut rope = ByteVec::new();
        let mut model: Vec<u8> = Vec::new();
        // Persistent aliases (a full clone + a zero-copy slice), refreshed by `TakeAliases`; the
        // per-op asserts below prove no later mutation of `rope` leaks into them.
        let mut alias: Option<(ByteVec, Vec<u8>)> = None;
        let mut sl: Option<(ByteVec, Vec<u8>)> = None;
        // A retained shared chunk (rc >= 2 once pushed): edits must COW, never write through it.
        let big = Bytes::from(alloc::vec![0x77u8; COW_SPLIT_ABOVE + 5]);
        for op in &ops {
            match op {
                Op::PushBack(d) => {
                    rope.push_back(Bytes::from(d.clone()));
                    model.extend_from_slice(d);
                }
                Op::PushFront(d) => {
                    rope.push_front(Bytes::from(d.clone()));
                    model.splice(0..0, d.iter().copied());
                }
                Op::PopFront => match rope.pop_front() {
                    Some(c) => {
                        assert_eq!(&c[..], &model[..c.len()]);
                        model.drain(..c.len());
                    }
                    None => assert!(model.is_empty()),
                },
                Op::PopBack => match rope.pop_back() {
                    Some(c) => {
                        let s = model.len() - c.len();
                        assert_eq!(&c[..], &model[s..]);
                        model.truncate(s);
                    }
                    None => assert!(model.is_empty()),
                },
                Op::Advance(n) => {
                    let k = n % (model.len() + 1);
                    rope.advance(k).unwrap();
                    model.drain(..k);
                }
                Op::Truncate(n) => {
                    let k = n % (model.len() + 1);
                    rope.truncate(k);
                    model.truncate(k);
                }
                Op::SplitTo(n) => {
                    let k = n % (model.len() + 1);
                    let front = rope.split_to(k).unwrap();
                    assert_eq!(front, &model[..k]);
                    model.drain(..k);
                }
                Op::SplitToCopy(n) => {
                    let k = n % (model.len() + 1);
                    let front = rope.split_to_copy(k).unwrap();
                    assert_eq!(&front[..], &model[..k]);
                    model.drain(..k);
                }
                Op::Append(d) => {
                    let mut other: ByteVec = d.chunks(3).map(Bytes::copy_from_slice).collect();
                    rope.append(&mut other);
                    assert!(other.is_empty());
                    model.extend_from_slice(d);
                }
                Op::AppendSelfSlice(a, b) => {
                    let lo = a % (model.len() + 1);
                    let hi = lo + b % (model.len() - lo + 1);
                    let mut other = rope.slice(lo..hi);
                    rope.append(&mut other);
                    assert!(other.is_empty());
                    model.extend_from_within(lo..hi);
                }
                Op::Slice(a, b) => {
                    let lo = a % (model.len() + 1);
                    let hi = lo + b % (model.len() - lo + 1);
                    assert_eq!(rope.slice(lo..hi), &model[lo..hi]);
                }
                Op::GetByte(i) => {
                    let idx = i % (model.len() + 1);
                    assert_eq!(rope.byte_at(idx), model.get(idx).copied());
                }
                Op::GetChunk(i) => {
                    // get(index) descends by CACHED chunk counts in the deep tier; the chunk
                    // iterator is its oracle (pointer identity, not just bytes).
                    let n = rope.chunks().len();
                    let idx = i % (n + 1);
                    match rope.get(idx) {
                        Some(c) => {
                            let it = rope.chunks().nth(idx).expect("iterator chunk");
                            assert_eq!(c.as_ptr(), it.as_ptr(), "get({idx}) wrong chunk");
                            assert_eq!(c.len(), it.len());
                        }
                        None => assert_eq!(idx, n, "get({idx}) None but {n} chunks"),
                    }
                }
                Op::SetByte(i, v) => {
                    if model.is_empty() {
                        assert_eq!(rope.set_byte(0, *v), Err(ByteVecError::OutOfBounds(0)));
                    } else {
                        let idx = i % model.len();
                        rope.set_byte(idx, *v).unwrap();
                        model[idx] = *v;
                    }
                }
                Op::Replace(a, b, d, kind) => {
                    let lo = a % (model.len() + 1);
                    let hi = lo + b % (model.len() - lo + 1);
                    // Exercise every reader-backed value kind: borrowed slice, owned Bytes,
                    // another ByteVec (zero-copy chunk splice), and an equal-length overwrite
                    // (the in-place path) whose value is exactly `hi - lo` bytes.
                    let repl: Vec<u8> = if kind % 4 == 3 {
                        (lo..hi).map(|k| (k as u8) ^ 0x5a).collect()
                    } else {
                        d.clone()
                    };
                    match kind % 4 {
                        0 => rope.replace(lo..hi, &repl[..]).unwrap(),
                        1 => rope.replace(lo..hi, Bytes::from(repl.clone())).unwrap(),
                        2 => {
                            let vr: ByteVec = repl.chunks(3).map(Bytes::copy_from_slice).collect();
                            rope.replace(lo..hi, vr).unwrap();
                        }
                        _ => rope.replace(lo..hi, &repl[..]).unwrap(),
                    }
                    model.splice(lo..hi, repl.iter().copied());
                }
                Op::BufRead(a, b) => {
                    // bytes::Buf on a shared clone; the original must be untouched (the global
                    // asserts below see any disturbance).
                    let mut r = rope.clone();
                    let k = a % (model.len() + 1);
                    bytes::Buf::advance(&mut r, k);
                    let m = b % (model.len() - k + 1);
                    let got = bytes::Buf::copy_to_bytes(&mut r, m);
                    assert_eq!(&got[..], &model[k..k + m]);
                }
                Op::FlattenCheck => {
                    assert_eq!(&rope.copy_to_bytes()[..], &model[..]);
                    assert_eq!(rope.chunks().len(), rope.chunks().count());
                    let flat: Vec<u8> = rope.chunks().flat_map(|c| c.iter().copied()).collect();
                    assert_eq!(flat, model);
                }
                Op::Deepen => {
                    // reach the deep tier (tree + buffered head) in ONE op, so short sequences
                    // exercise deep paths without needing PROMOTE_AT+ pushes
                    for i in 0..PROMOTE_AT {
                        let b = [(i % 251) as u8, (i % 239) as u8];
                        rope.push_back(chunk(&b));
                        model.extend_from_slice(&b);
                    }
                    for i in 0..3u8 {
                        let b = [0xB0 ^ i; 2];
                        rope.push_front(chunk(&b));
                        model.splice(0..0, b.iter().copied());
                    }
                }
                Op::Clear => {
                    rope.clear();
                    model.clear();
                }
                Op::TakeAliases(a, b) => {
                    alias = Some((rope.clone(), model.clone()));
                    let lo = a % (model.len() + 1);
                    let hi = lo + b % (model.len() - lo + 1);
                    sl = Some((rope.slice(lo..hi), model[lo..hi].to_vec()));
                }
                Op::MutateAlias(i, v) => {
                    // write THROUGH the zero-copy slice alias; the parent rope must be intact
                    // (the global rope == model assert below proves it)
                    if let Some((srope, smodel)) = sl.as_mut().filter(|(_, m)| !m.is_empty()) {
                        let idx = i % smodel.len();
                        srope.set_byte(idx, *v).unwrap();
                        smodel[idx] = *v;
                    }
                }
                Op::PushShared => {
                    rope.push_back(big.clone());
                    model.extend_from_slice(&big);
                }
                Op::StartsEndsWith(k, d) => {
                    // chunk-aware starts_with/ends_with (#58) vs the model: a REAL prefix/suffix
                    // always matches, its single-byte perturbation never does, and a random
                    // literal agrees with the slice oracle.
                    let k = k % (model.len() + 1);
                    assert!(rope.starts_with(&model[..k]), "own prefix len {k}");
                    assert!(
                        rope.ends_with(&model[model.len() - k..]),
                        "own suffix len {k}"
                    );
                    if k > 0 {
                        let mut p = model[..k].to_vec();
                        p[k - 1] = p[k - 1].wrapping_add(1);
                        assert!(!rope.starts_with(&p), "perturbed prefix matched, len {k}");
                        let mut s = model[model.len() - k..].to_vec();
                        s[0] = s[0].wrapping_add(1);
                        assert!(!rope.ends_with(&s), "perturbed suffix matched, len {k}");
                    }
                    assert_eq!(rope.starts_with(d), model.starts_with(&d[..]));
                    assert_eq!(rope.ends_with(d), model.ends_with(&d[..]));
                }
                Op::DoubleEndedCheck => {
                    // The chunk iterator's DoubleEndedIterator (front + back cursors) must partition
                    // the chunks with pointer identity and exact len on EVERY shape the harness
                    // reaches (deep starts, bulk builds, post-split, aliased). Two properties:
                    // (a) `.rev()` == forward reversed; (b) interleaved next/next_back yields each
                    // chunk exactly once (front-prefix ++ reversed back-suffix == forward).
                    let fwd: Vec<&Bytes> = rope.chunks().collect();
                    assert_eq!(rope.chunks().rev().len(), fwd.len());
                    let mut rev: Vec<&Bytes> = rope.chunks().rev().collect();
                    rev.reverse();
                    assert_eq!(rev.len(), fwd.len(), "chunks().rev() count mismatch");
                    for (i, (a, b)) in rev.iter().zip(fwd.iter()).enumerate() {
                        assert_eq!(a.as_ptr(), b.as_ptr(), "chunks().rev()[{i}] wrong chunk");
                        assert_eq!(a.len(), b.len());
                    }
                    let mut it = rope.chunks();
                    let mut front: Vec<*const u8> = Vec::new();
                    let mut back: Vec<*const u8> = Vec::new();
                    let mut take_front = true;
                    loop {
                        let got = if take_front {
                            it.next()
                        } else {
                            it.next_back()
                        };
                        match got {
                            Some(c) if take_front => front.push(c.as_ptr()),
                            Some(c) => back.push(c.as_ptr()),
                            None => break,
                        }
                        take_front = !take_front;
                    }
                    front.extend(back.into_iter().rev());
                    let want: Vec<*const u8> = fwd.iter().map(|c| c.as_ptr()).collect();
                    assert_eq!(
                        front, want,
                        "interleaved front/back must partition the chunks"
                    );
                }
                Op::InterleavedChunksCheck(pattern) => {
                    // Fuzz-CHOSEN front/back interleave (the fixed alternation in DoubleEndedCheck
                    // always meets near the middle chunk): an arbitrary pattern moves the meet point
                    // into every sub-iterator — inside the head deque, mid-leaf in the tree, in the
                    // tail — and back-first starts exercise the lazy back-cursor build. After the
                    // ends meet, BOTH must keep returning None.
                    let want: Vec<*const u8> = rope.chunks().map(|c| c.as_ptr()).collect();
                    let mut it = rope.chunks();
                    let mut front: Vec<*const u8> = Vec::new();
                    let mut back: Vec<*const u8> = Vec::new();
                    let mut len = want.len();
                    for take_front in pattern
                        .iter()
                        .copied()
                        .chain([true, false].into_iter().cycle())
                    {
                        assert_eq!(it.len(), len, "len must track both-ends consumption");
                        let got = if take_front {
                            it.next()
                        } else {
                            it.next_back()
                        };
                        match got {
                            Some(c) if take_front => front.push(c.as_ptr()),
                            Some(c) => back.push(c.as_ptr()),
                            None => break,
                        }
                        len -= 1;
                    }
                    assert_eq!(it.len(), 0, "iterator reported None while len > 0");
                    assert!(it.next().is_none(), "front must stay None after the meet");
                    assert!(
                        it.next_back().is_none(),
                        "back must stay None after the meet"
                    );
                    front.extend(back.into_iter().rev());
                    assert_eq!(
                        front, want,
                        "fuzz-interleaved front/back must partition the chunks"
                    );
                }
                Op::AsContiguousCheck => {
                    // The only soundness face of as_contiguous: a Some view must be the ENTIRE
                    // content (never a partial chunk of a multi-chunk rope), on every shape the
                    // harness reaches. None is always a legal (conservative) answer.
                    if let Some(view) = rope.as_contiguous() {
                        assert_eq!(view.len(), rope.len(), "as_contiguous length");
                        assert_eq!(view, &model[..], "as_contiguous content");
                        assert_eq!(rope.chunks().len(), usize::from(!view.is_empty()));
                    }
                }
            }
            assert_eq!(rope.len(), model.len(), "len after {op:?}");
            assert_eq!(rope, model, "bytes after {op:?}");
            if let Some((arope, amodel)) = &alias {
                assert_eq!(arope, amodel, "persistent clone disturbed after {op:?}");
            }
            if let Some((srope, smodel)) = &sl {
                assert_eq!(srope, smodel, "persistent slice disturbed after {op:?}");
            }
        }
        assert!(
            big.iter().all(|&b| b == 0x77),
            "shared Bytes handle was written through"
        );
    });
}

/// A BULK-BUILT rope (`FromIterator` with a size hint above `PROMOTE_AT` takes the bottom-up
/// `extend_blocks` + `from_tree` path, #31) must agree with the flat oracle on every axis the
/// incremental `push_back` construction does: bytes, `get(index)` vs the chunk iterator (cached
/// chunk counts), `byte_at`, `slice`, and `split_to` through the tree-body fast path (#30).
#[test]
fn bulk_built_rope_matches_oracle_on_every_axis() {
    // exact size hint (Vec iterator) above PROMOTE_AT -> the bulk bottom-up build path
    let chunks: Vec<Bytes> = (0..(PROMOTE_AT * 3 + 7))
        .map(|i| Bytes::from(alloc::vec![(i % 251) as u8; 1 + i % 5]))
        .collect();
    let flat: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
    let rope: ByteVec = chunks.iter().cloned().collect();
    assert!(
        matches!(rope.repr, Repr::Deep(_)),
        "bulk collect should land deep"
    );
    rope.check_invariants();
    assert_eq!(rope, flat, "bulk-built bytes");

    // get(index) vs the chunk iterator: pointer identity for every index
    let n = rope.chunks().len();
    assert_eq!(n, chunks.len(), "bulk build dropped or merged chunks");
    for i in 0..n {
        let via_iter = rope.chunks().nth(i).expect("iterator chunk");
        let via_get = rope.get(i).expect("get chunk");
        assert_eq!(
            via_get.as_ptr(),
            via_iter.as_ptr(),
            "bulk get({i}) wrong chunk"
        );
    }
    assert!(rope.get(n).is_none());

    // byte_at + slice spot grid
    for &off in &[0, 1, flat.len() / 3, flat.len() / 2, flat.len() - 1] {
        assert_eq!(rope.byte_at(off), Some(flat[off]), "bulk byte_at {off}");
    }
    for &(a, b) in &[
        (0, flat.len()),
        (3, flat.len() / 2),
        (flat.len() / 3, flat.len() - 2),
    ] {
        assert_eq!(rope.slice(a..b), &flat[a..b], "bulk slice {a}..{b}");
    }

    // split_to through the tree body (both halves oracle-exact, invariants hold)
    for at in [1, flat.len() / 3, flat.len() / 2, flat.len() - 1] {
        let mut r = rope.clone();
        let front = r.split_to(at).unwrap();
        front.check_invariants();
        r.check_invariants();
        assert_eq!(front, &flat[..at], "bulk split front at {at}");
        assert_eq!(r, &flat[at..], "bulk split back at {at}");
    }
}

/// Deep-tier `get(index)` descends the tree by the per-subtree CACHED chunk counts, so any stale
/// count fix-up (leaf splits from bounded-COW `set_byte`, concat seam repacks, pop-block refills,
/// structural `replace`) would silently send it to the WRONG chunk while the byte content stays
/// right. Oracle: `get(i)` must equal `chunks().nth(i)` (pointer + bytes) for EVERY index, swept
/// after each count-perturbing mutation, with all three regions (head/tree/tail) populated.
#[test]
fn get_index_matches_chunk_iterator_after_count_perturbing_mutations() {
    fn sweep(rope: &ByteVec, label: &str) {
        let n = rope.chunks().len();
        for i in 0..n {
            let via_iter = rope.chunks().nth(i).expect("iterator chunk");
            let via_get = rope.get(i).expect("get chunk");
            assert_eq!(
                via_get.as_ptr(),
                via_iter.as_ptr(),
                "{label}: get({i}) returned a different chunk than chunks().nth({i})"
            );
            assert_eq!(via_get.len(), via_iter.len(), "{label}: get({i}) length");
        }
        assert!(rope.get(n).is_none(), "{label}: get(count) must be None");
        assert!(
            rope.get(n + 1000).is_none(),
            "{label}: get(far) must be None"
        );
    }

    // All three regions populated: tree via push_back (incl. large shared chunks that will split
    // on set_byte), buffered head via push_front, buffered tail via trailing push_backs.
    let big = Bytes::from(alloc::vec![9u8; COW_SPLIT_ABOVE * 2 + 1]);
    let mut rope = ByteVec::new();
    for i in 0..(PROMOTE_AT * 2) {
        if i % 11 == 0 {
            rope.push_back(big.clone()); // shared: a later set_byte splits its leaf
        } else {
            rope.push_back(Bytes::from(alloc::vec![i as u8; 1 + i % 5]));
        }
    }
    for i in 0..7 {
        rope.push_front(Bytes::from(alloc::vec![0xA0u8 ^ i; 2]));
        rope.push_back(Bytes::from(alloc::vec![0x50u8 ^ i; 3]));
    }
    assert!(matches!(rope.repr, Repr::Deep(_)));
    sweep(&rope, "initial");

    // Leaf-splitting set_byte edits (shared big chunks -> bounded COW split -> count fix-ups up
    // the spine, possibly leaf/branch/root splits).
    let len = rope.len();
    for k in 0..40usize {
        let off = (k * 6151 + 13) % len;
        rope.set_byte(off, 0xEE).unwrap();
    }
    sweep(&rope, "after set_byte splits");

    // Structural replace (UC6 tree splice: split + concat seam repacks).
    let l = rope.len();
    rope.replace(l / 3..l / 2, Bytes::from(alloc::vec![0x33u8; 97]))
        .unwrap();
    sweep(&rope, "after structural replace");

    // End churn: pop refills from tree blocks at both ends, then re-push.
    for _ in 0..12 {
        rope.pop_front();
        rope.pop_back();
    }
    sweep(&rope, "after end pops");
    for i in 0..12u8 {
        rope.push_front(Bytes::from(alloc::vec![i | 0x80; 2]));
        rope.push_back(Bytes::from(alloc::vec![i | 0x40; 2]));
    }
    sweep(&rope, "after re-push");

    // Concat of two deep ropes (seam repack merges leaf blocks -> counts recomputed).
    let mut other = ByteVec::new();
    for i in 0..(PROMOTE_AT * 2) {
        other.push_back(Bytes::from(alloc::vec![i as u8 ^ 0xFF; 1 + i % 3]));
    }
    rope.append(&mut other);
    sweep(&rope, "after deep concat");

    // Self-sharing append (slice + append shares subtrees with re-counted spines).
    let quarter = rope.len() / 4;
    let mut part = rope.slice(quarter..quarter * 3);
    rope.append(&mut part);
    sweep(&rope, "after self-slice append");

    // split_to leaves both halves with fresh spines.
    let front = rope.split_to(rope.len() / 2).unwrap();
    sweep(&front, "split front");
    sweep(&rope, "split back");
}

/// Differential oracle for the [`Builder`] construction surface (bytevec-compat), reusing the same
/// `Vec<u8>`-model discipline as [`differential_against_model`]. Drives a `Builder` through a random
/// op sequence (the `writer::Buffer` write path, `append`/`extend`/`split`/`split_to`/
/// `with_inline_threshold`/`write_with_len_prefix`) and checks `len`/`is_empty` after every op and the
/// final `finish()` bytes against the model. Extend `BuilderOp` here rather than adding a one-off
/// harness when covering a new Builder method.
#[test]
fn builder_differential_against_model() {
    use bolero::check;
    use bolero_generator::TypeGenerator;
    use etude_buffer::writer::Buffer as _;

    #[derive(Debug, Clone, TypeGenerator)]
    enum BuilderOp {
        PutSlice(Vec<u8>),
        PutBytes(Vec<u8>),
        PutBytesMut(Vec<u8>),
        Append(Vec<u8>),
        Extend(Vec<u8>),
        SplitTo(usize),
        Split,
        WriteWithLenPrefix(Vec<u8>),
        SetInlineThreshold(usize),
        SocketRead(Vec<u8>, u8),
        ReadChunk(usize),
        NestedLenPrefix(Vec<u8>),
    }

    check!()
        .with_type::<(usize, Vec<BuilderOp>)>()
        .cloned()
        .for_each(|(cap, ops)| {
            // vary the head-buffer capacity so both the buffered and flush-to-chunk paths are hit
            let capacity = 1 + cap % 64;
            let mut builder = ByteVec::builder(capacity);
            let mut model: Vec<u8> = Vec::new();
            for op in &ops {
                match op {
                    BuilderOp::PutSlice(d) => {
                        builder.put_slice(d);
                        model.extend_from_slice(d);
                    }
                    BuilderOp::PutBytes(d) => {
                        builder.put_bytes(Bytes::from(d.clone()));
                        model.extend_from_slice(d);
                    }
                    BuilderOp::PutBytesMut(d) => {
                        builder.put_bytes_mut(BytesMut::from(&d[..]));
                        model.extend_from_slice(d);
                    }
                    BuilderOp::Append(d) => {
                        let mut other: ByteVec = d.chunks(3).map(Bytes::copy_from_slice).collect();
                        builder.append(&mut other);
                        model.extend_from_slice(d);
                    }
                    BuilderOp::Extend(d) => {
                        let other: ByteVec = d.chunks(3).map(Bytes::copy_from_slice).collect();
                        builder.extend(&other);
                        model.extend_from_slice(d);
                    }
                    BuilderOp::SplitTo(n) => {
                        let k = n % (model.len() + 1);
                        let front = builder.split_to(k).unwrap();
                        assert_eq!(front, &model[..k], "split_to({k}) front");
                        model.drain(..k);
                    }
                    BuilderOp::Split => {
                        let taken = builder.split();
                        assert_eq!(taken, &model[..], "split takes all");
                        model.clear();
                        assert!(builder.is_empty());
                    }
                    BuilderOp::WriteWithLenPrefix(d) => {
                        builder.write_with_len_prefix(|w| w.put_slice(d));
                        model.extend_from_slice(&(d.len() as u64).to_be_bytes());
                        model.extend_from_slice(d);
                    }
                    BuilderOp::SetInlineThreshold(t) => {
                        builder = builder.with_inline_threshold(t % 32);
                    }
                    BuilderOp::SocketRead(d, extra) => {
                        // A well-behaved socket read: asks for a few bytes more than it fills
                        // (exercising flush_and_reserve and short reads), fills a prefix, and
                        // reports exactly what it filled.
                        let preferred = d.len() + usize::from(extra % 8);
                        builder.for_socket_read(preferred, |slice| {
                            slice[..d.len()].copy_from_slice(d);
                            d.len()
                        });
                        model.extend_from_slice(d);
                    }
                    BuilderOp::ReadChunk(n) => {
                        use etude_buffer::reader::Buffer as _;
                        let watermark = n % (model.len() + 2);
                        let chunk = builder.read_chunk(watermark).unwrap();
                        assert!(chunk.len() <= watermark, "read_chunk over watermark");
                        assert_eq!(&chunk[..], &model[..chunk.len()], "read_chunk front bytes");
                        let taken = chunk.len();
                        model.drain(..taken);
                    }
                    BuilderOp::NestedLenPrefix(d) => {
                        builder.write_with_len_prefix(|w| {
                            w.put_slice(d);
                            w.write_with_len_prefix(|w2| w2.put_slice(d));
                        });
                        let inner_len = d.len() as u64;
                        let outer_len = (d.len() * 2 + 8) as u64;
                        model.extend_from_slice(&outer_len.to_be_bytes());
                        model.extend_from_slice(d);
                        model.extend_from_slice(&inner_len.to_be_bytes());
                        model.extend_from_slice(d);
                    }
                }
                assert_eq!(builder.len(), model.len(), "len after {op:?}");
                assert_eq!(
                    builder.is_empty(),
                    model.is_empty(),
                    "is_empty after {op:?}"
                );
            }
            let built = builder.finish();
            assert_eq!(built, model, "finish bytes");
        });
}

#[test]
fn advance_across_deep_tree() {
    let n = PROMOTE_AT * 3;
    let mut rope = ByteVec::new();
    let mut expected = Vec::new();
    for i in 0..n {
        let b = [(i % 251) as u8; 4];
        rope.push_back(chunk(&b));
        expected.extend_from_slice(&b);
    }
    // consume a prime-sized bite repeatedly and compare the tail each time
    let mut consumed = 0;
    while consumed < expected.len() {
        let step = 37.min(expected.len() - consumed);
        rope.advance(step).unwrap();
        consumed += step;
        assert_eq!(rope, &expected[consumed..]);
    }
    assert!(rope.is_empty());
}

/// Editing one byte of a *large shared* chunk must NOT copy the whole chunk: the chunk is split into a
/// shared prefix + the one owned edited byte + a shared suffix. We prove the prefix/suffix are O(1)
/// views of the original allocation via pointer identity, and that the shared original is untouched.
#[test]
fn set_byte_on_large_shared_chunk_splits_instead_of_copying() {
    let big = Bytes::from(alloc::vec![7u8; COW_SPLIT_ABOVE * 4]);
    let base = big.as_ptr();
    let mut rope = ByteVec::new();
    rope.push_back(big.clone()); // rc >= 2: `big` + the rope share the allocation

    let at = COW_SPLIT_ABOVE * 2 + 5;
    rope.set_byte(at, 0xFF).unwrap();

    // Content correct; the shared original is untouched (copy-on-write).
    assert_eq!(rope.byte_at(at), Some(0xFF));
    assert_eq!(rope.byte_at(0), Some(7));
    assert_eq!(big[at], 7);

    // Proof of bounded copy: prefix and suffix are shared slices of `big` (same backing pointer); only
    // the 1-byte middle is freshly owned.
    let chunks: alloc::vec::Vec<&Bytes> = rope.chunks().collect();
    assert_eq!(
        chunks.len(),
        3,
        "chunk split into prefix / edited byte / suffix"
    );
    assert_eq!(chunks[0].as_ptr(), base);
    assert_eq!(chunks[0].len(), at);
    assert_eq!(chunks[1].len(), 1);
    assert_eq!(chunks[2].as_ptr(), unsafe { base.add(at + 1) });

    let mut expect = alloc::vec![7u8; COW_SPLIT_ABOVE * 4];
    expect[at] = 0xFF;
    assert_eq!(rope, expect);
    rope.check_invariants();
}

/// A shared chunk at or below the threshold is copied whole (one chunk, no fragmentation); a unique
/// chunk of any size is edited fully in place (no split, no copy).
#[test]
fn set_byte_small_shared_copies_whole_and_unique_edits_in_place() {
    // Small shared chunk -> whole copy, stays one chunk.
    let small = Bytes::from(alloc::vec![1u8; COW_SPLIT_ABOVE]);
    let mut rope = ByteVec::new();
    rope.push_back(small.clone());
    rope.set_byte(10, 2).unwrap();
    assert_eq!(
        rope.chunks().count(),
        1,
        "small shared chunk stays one chunk"
    );
    assert_eq!(small[10], 1, "original untouched");

    // Large UNIQUE chunk -> in place, still one chunk (try_into_mut succeeds).
    let mut rope2 = ByteVec::new();
    rope2.push_back(Bytes::from(alloc::vec![3u8; COW_SPLIT_ABOVE * 4]));
    let ptr = rope2.get(0).unwrap().as_ptr();
    rope2.set_byte(999, 4).unwrap();
    assert_eq!(rope2.chunks().count(), 1, "unique chunk edited in place");
    assert_eq!(
        rope2.get(0).unwrap().as_ptr(),
        ptr,
        "same allocation, mutated in place"
    );
    assert_eq!(rope2.byte_at(999), Some(4));
}

/// `replace` (equal-length overwrite) of a small span inside a large shared chunk is likewise bounded:
/// the untouched prefix/suffix are shared, only the overwritten span is materialized.
#[test]
fn replace_small_span_in_large_shared_chunk_is_bounded() {
    let big = Bytes::from(alloc::vec![0u8; COW_SPLIT_ABOVE * 4]);
    let base = big.as_ptr();
    let mut rope = ByteVec::new();
    rope.push_back(big.clone());

    let at = COW_SPLIT_ABOVE * 2;
    rope.replace(at..at + 4, &b"abcd"[..]).unwrap();

    let chunks: alloc::vec::Vec<&Bytes> = rope.chunks().collect();
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].as_ptr(), base);
    assert_eq!(chunks[0].len(), at);
    assert_eq!(&chunks[1][..], b"abcd");
    assert_eq!(chunks[2].as_ptr(), unsafe { base.add(at + 4) });
    assert_eq!(big[at], 0, "shared original untouched");
    rope.check_invariants();
}

/// Bounded copy-on-write in the DEEP tier: large (>4 KiB) shared chunks living inside the tree are
/// split on edit, which grows leaf blocks and — on overflow — splits leaves and propagates the split
/// up the spine. `check_invariants` validates the whole rebalance; the shared snapshot proves COW.
#[test]
fn set_byte_bounded_cow_in_tree_rebalances_and_preserves_sharing() {
    let mut rope = ByteVec::new();
    let mut flat: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    for i in 0..80u32 {
        let (len, val) = if i % 7 == 0 {
            (COW_SPLIT_ABOVE * 2 + 3, i as u8) // big: lands in the tree once flushed
        } else {
            (5usize, i as u8)
        };
        rope.push_back(Bytes::from(alloc::vec![val; len]));
        flat.resize(flat.len() + len, val);
    }
    assert!(matches!(rope.repr, Repr::Deep(_)));

    let snapshot = rope.clone(); // shares every chunk -> every edit is copy-on-write
    let orig_flat = flat.clone();

    for &off in &[
        3usize,
        flat.len() / 3,
        flat.len() / 2,
        COW_SPLIT_ABOVE + 1,
        flat.len() - 1,
    ] {
        rope.set_byte(off, 0xEE).unwrap();
        flat[off] = 0xEE;
        rope.check_invariants(); // catches any bad cache/height/fanout after leaf & branch splits
    }
    assert_eq!(rope, flat);
    assert_eq!(snapshot, orig_flat, "COW: the shared snapshot is untouched");
}

/// Stress the leaf→branch→root split propagation: a multi-level tree of exclusively large shared
/// chunks, where every edit splits a leaf. Enough edits overflow leaves into branch splits and grow
/// the root's height. `check_invariants` validates fanout, cached sizes/totals/counts, and height at
/// every step; the untouched snapshot confirms nothing aliased through the rebalances.
#[test]
fn set_byte_tree_split_propagation_stress() {
    let clen = COW_SPLIT_ABOVE + 1; // just over threshold -> always splits when shared
    let n = FANOUT * FANOUT + 40; // forces a height >= 2 tree with a wide root branch
    let mut rope = ByteVec::new();
    for i in 0..n {
        rope.push_back(Bytes::from(alloc::vec![(i % 251) as u8; clen]));
    }
    assert!(matches!(rope.repr, Repr::Deep(_)));

    let snapshot = rope.clone(); // every chunk shared -> every edit is a bounded COW split
    let base_len = rope.len();

    // Edit a byte in ~60 chunks spread across the whole tree; each shared-chunk edit splits its leaf.
    for k in 0..60usize {
        let off = (k * clen * 17 + 3) % base_len; // scattered, deterministic
        rope.set_byte(off, 0xC3).unwrap();
        rope.check_invariants(); // fanout / cache / height must hold after each split cascade
        assert_eq!(rope.byte_at(off), Some(0xC3), "edit {k} at {off}");
    }
    // Length is invariant under set_byte; the shared original never changed a byte (COW).
    assert_eq!(rope.len(), base_len);
    assert_eq!(snapshot.len(), base_len);
    assert_eq!(snapshot.byte_at(3), Some(0));
}

/// RED reproducer (breaker-bytevec): an equal-length `replace` of a SMALL span inside a large
/// SHARED chunk that lives in the TREE copies the WHOLE chunk — `Node::overwrite`'s leaf arm calls
/// `overwrite_one`, whose shared path is `BytesMut::from(&shared[..])` (unbounded). The flat tier
/// and the deep head/tail deques use the bounded `cow_edit` split instead (shared prefix/suffix,
/// copy bounded by the edited span — pointer-identity-proven by
/// `replace_small_span_in_large_shared_chunk_is_bounded`), and tree `set_byte` is ALREADY bounded.
/// Observed: a 4-byte overwrite of a shared 16 KiB tree chunk leaves 0 chunks sharing the original
/// allocation (whole chunk copied); a 1-byte overwrite of a shared 1 GiB tree chunk would copy
/// 1 GiB. Expected (parity with every other tier): shared prefix/suffix views remain. Fix shape:
/// switch `Node::overwrite`'s leaf arm to `cow_edit` + the existing `finish_leaf`/`InsertResult`
/// split plumbing (byte totals conserved; only chunk counts change).
#[test]
fn tree_overwrite_small_span_in_large_shared_chunk_is_bounded() {
    // A large shared chunk that lands INSIDE the tree (not the buffered ends).
    let big = Bytes::from(alloc::vec![7u8; COW_SPLIT_ABOVE * 4]);
    let base = big.as_ptr() as usize;
    let mut rope = ByteVec::new();
    for i in 0..(PROMOTE_AT + 1) {
        if i == 5 {
            rope.push_back(big.clone());
        } else {
            rope.push_back(Bytes::from(alloc::vec![i as u8; 4]));
        }
    }
    assert!(matches!(rope.repr, Repr::Deep(_)));
    let at = 5 * 4 + COW_SPLIT_ABOVE; // inside the big chunk
    // Equal-length overwrite of 4 bytes (UC1-3 path -> Node::overwrite in the tree).
    rope.replace(at..at + 4, &b"abcd"[..]).unwrap();
    // Content must be right regardless (and is — this part passes today).
    assert_eq!(rope.byte_at(at), Some(b'a'));
    assert_eq!(rope.byte_at(at + 4), Some(7));
    assert_eq!(big[at], 7, "shared original untouched");
    // Bounded COW leaves shared prefix/suffix views into big's allocation, as the flat tier does.
    let shared_views = rope
        .chunks()
        .filter(|c| {
            let p = c.as_ptr() as usize;
            p >= base && p < base + COW_SPLIT_ABOVE * 4
        })
        .count();
    assert!(
        shared_views > 0,
        "tree-level equal-length overwrite copied the WHOLE shared chunk (unbounded COW)"
    );
}

/// The public trait impls callers rely on: `From<String>`, `Extend<Vec<u8>>`, the
/// `PartialEq<[Bytes]>` family, and `From<ByteVecError> for std::io::Error`.
#[test]
fn public_trait_impl_surface() {
    // From<String>
    let from_string = ByteVec::from(String::from("hello"));
    assert_eq!(from_string, b"hello");

    // Extend<Vec<u8>>
    let mut r = ByteVec::from(b"a");
    r.extend([alloc::vec![b'b', b'c'], alloc::vec![b'd']]);
    assert_eq!(r, b"abcd");

    // PartialEq<[Bytes]> / <&[Bytes]> / <[Bytes; N]> / <&[Bytes; N]> — chunking-independent.
    let rope: ByteVec = [chunk(b"foo"), chunk(b"bar")].into_iter().collect();
    let chunks_arr = [Bytes::from_static(b"foo"), Bytes::from_static(b"bar")];
    assert_eq!(rope, chunks_arr); // [Bytes; N]
    assert_eq!(rope, &chunks_arr); // &[Bytes; N]
    assert_eq!(rope, chunks_arr[..]); // [Bytes]
    assert_eq!(rope, &chunks_arr[..]); // &[Bytes]
    // Different chunking, same bytes, still equal (content compare, not chunk-identity).
    let one_chunk = [Bytes::from_static(b"foobar")];
    assert_eq!(rope, one_chunk[..]);
    // Inequality is detected.
    let wrong = [Bytes::from_static(b"foo"), Bytes::from_static(b"baz")];
    assert!(rope != wrong);

    // From<ByteVecError> for std::io::Error maps OutOfBounds -> UnexpectedEof.
    let io_err: std::io::Error = ByteVecError::OutOfBounds(7).into();
    assert_eq!(io_err.kind(), std::io::ErrorKind::UnexpectedEof);
}

mod tag_macro_expands {
    // The `static_bytevec_tag!` macro must expand at an arbitrary module path.
    crate::static_bytevec_tag!(crate::tagged);
}

/// The public type names all resolve: `ByteVec` / `ByteVecError` / `ChunkIter` / `DrainIter` /
/// `static_bytevec_tag!` are all reachable from the crate root.
#[test]
fn public_names_resolve() {
    // ByteVec == ByteVec
    let v: crate::ByteVec = crate::ByteVec::from(b"hello");
    assert_eq!(v, b"hello");

    // ByteVecError == ByteVecError
    let e: crate::ByteVecError = crate::ByteVecError::OutOfBounds(3);
    assert_eq!(e, ByteVecError::OutOfBounds(3));

    // ChunkIter<'_> == Chunks<'_>
    let rope: ByteVec = [chunk(b"ab"), chunk(b"cd")].into_iter().collect();
    let it: crate::ChunkIter<'_> = rope.chunks();
    assert_eq!(it.count(), 2);

    // DrainIter == IntoChunks
    let di: crate::DrainIter = rope.into_iter();
    assert_eq!(di.count(), 2);

    // static_bytevec_tag! machinery tracks the byte budget.
    let tagged: Tagged<tag_macro_expands::Tag> = ByteVec::from(b"hi!").tag(&tag_macro_expands::Tag);
    assert_eq!(tag_macro_expands::Tag::current(), 3);
    drop(tagged);
    assert_eq!(tag_macro_expands::Tag::current(), 0);
}
