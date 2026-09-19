// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! The deep tier's relaxed-radix tree of chunk *blocks*.
//!
//! A leaf is a *block* of `1..=FANOUT` [`Bytes`] chunks; interior branches hold `1..=FANOUT`
//! children. Every node sits behind an [`Arc`], so cloning the tree (and therefore a `Deep`
//! [`crate::ByteVec`]) is O(1) and shares structure. Nodes are **relaxed**: each branch carries a
//! per-child byte-size table plus cached `total` bytes and `count` chunks, and is keyed by child
//! *count* rather than a strict left-full radix invariant.
//!
//! ## Operations
//!
//! - `push_block` / `pop_*_block`: O(1) amortized at both ends. Children live in a [`VecDeque`], and
//!   uniquely-owned nodes are mutated in place (FBIP, via `Arc::get_mut`); only a shared tree
//!   path-copies the touched spine.
//! - `concat`: merges two trees along the seam (the rightmost path of the left, leftmost of the
//!   right) and repacks only those O(height) nodes — the bulk of both trees stays shared. O(log₃₂).
//! - `split`: descends the single child straddling the offset, sharing every subtree to its left/
//!   right wholesale. O(log₃₂).
//! - `byte_at`: O(log₃₂) size-table descent.

use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};
use bytes::Bytes;

use crate::FANOUT;

#[derive(Clone)]
struct Block {
    chunks: Box<[Bytes]>,
    bytes: usize,
}

#[derive(Clone)]
struct Branch {
    /// `1..=FANOUT` children, in a `VecDeque` (reserved to `FANOUT`) for O(1) push/pop at both ends.
    children: VecDeque<Node>,
    /// Per-child byte sizes: `sizes[i] == children[i].byte_len()`. Parallel to `children`.
    sizes: VecDeque<usize>,
    /// Cached total bytes (`sum(sizes)`) — O(1) `byte_len`.
    total: usize,
    /// Cached total chunk count (`sum(children[i].chunk_len())`) — O(1) `chunk_len`, needed so
    /// `split` can size each half without walking.
    count: usize,
}

#[derive(Clone)]
enum Node {
    Leaf(Arc<Block>),
    Branch(Arc<Branch>),
}

/// Outcome of an in-place push attempt.
enum Push {
    Placed,
    Full,
    Shared,
}

/// Outcome of a byte total-preserving edit that may have grown chunk counts and split a node — a
/// B-tree-style insert result threaded back up the spine (see [`Node::set_byte`]).
struct InsertResult {
    /// Net chunks added to this subtree (0, 1, or 2), for ancestor `count` fix-up.
    added: usize,
    /// A right sibling to insert immediately after this node in its parent (the node split); the node
    /// itself has already shrunk. `total` bytes are conserved across `self` + `overflow`.
    overflow: Option<Node>,
}

impl InsertResult {
    #[inline]
    fn none() -> Self {
        InsertResult {
            added: 0,
            overflow: None,
        }
    }
}

/// Installs the (possibly grown) `chunks` into `block`, splitting the leaf when it overflows `FANOUT`
/// and returning the overflow sibling. Byte total is conserved, so an un-split block keeps its cached
/// `bytes`; a split recomputes the retained left portion's total.
fn finish_leaf(block: &mut Block, mut chunks: Vec<Bytes>, added: usize) -> InsertResult {
    if chunks.len() <= FANOUT {
        block.chunks = chunks.into_boxed_slice();
        InsertResult {
            added,
            overflow: None,
        }
    } else {
        let right = chunks.split_off(FANOUT);
        block.bytes = chunks.iter().map(|c| c.len()).sum();
        block.chunks = chunks.into_boxed_slice();
        InsertResult {
            added,
            overflow: Some(Node::leaf(right)),
        }
    }
}

/// Applies a [`crate::CowEdit`] to chunk `i` of a leaf's chunk vec, splicing in the extra pieces of a
/// split. Returns the number of chunks added (0, 1, or 2); slot `i` must already have been consumed
/// (taken). Mirrors `splice_cow_deque` (lib.rs) for a `Vec` leaf block.
fn splice_cow_vec(chunks: &mut Vec<Bytes>, i: usize, edit: crate::CowEdit) -> usize {
    match edit {
        crate::CowEdit::InPlace(c) => {
            chunks[i] = c;
            0
        }
        crate::CowEdit::Split {
            prefix,
            mid,
            suffix,
        } => {
            chunks[i] = mid;
            let mut added = 0;
            if let Some(s) = suffix {
                chunks.insert(i + 1, s);
                added += 1;
            }
            if let Some(p) = prefix {
                chunks.insert(i, p); // pushes `mid` (and any suffix) right by one
                added += 1;
            }
            added
        }
    }
}

/// Absorbs a child's [`InsertResult`] into `branch` at child index `i`: bumps the chunk `count`, and
/// when the child split, fixes the boundary `sizes` and inserts the sibling, splitting this branch (and
/// bubbling a new overflow) if it exceeds `FANOUT`. `total` is unchanged (bytes are conserved).
fn absorb_child_insert(branch: &mut Branch, i: usize, res: InsertResult) -> InsertResult {
    branch.count += res.added;
    if let Some(sib) = res.overflow {
        branch.sizes[i] = branch.children[i].byte_len();
        let sib_bytes = sib.byte_len();
        branch.children.insert(i + 1, sib);
        branch.sizes.insert(i + 1, sib_bytes);
        if branch.children.len() > FANOUT {
            return InsertResult {
                added: res.added,
                overflow: Some(split_branch_off(branch)),
            };
        }
    }
    InsertResult {
        added: res.added,
        overflow: None,
    }
}

/// Splits an over-full branch at `FANOUT`, moving the right half into a new sibling branch and
/// recomputing both halves' cached `total`/`count`.
fn split_branch_off(branch: &mut Branch) -> Node {
    let right_children = branch.children.split_off(FANOUT);
    let right_sizes = branch.sizes.split_off(FANOUT);
    let right_total: usize = right_sizes.iter().sum();
    let right_count: usize = right_children.iter().map(|c| c.chunk_len()).sum();
    branch.total -= right_total;
    branch.count -= right_count;
    Node::Branch(Arc::new(Branch {
        children: right_children,
        sizes: right_sizes,
        total: right_total,
        count: right_count,
    }))
}

fn new_children() -> VecDeque<Node> {
    VecDeque::with_capacity(FANOUT)
}

impl Node {
    fn leaf(chunks: Vec<Bytes>) -> Node {
        let bytes = chunks.iter().map(|c| c.len()).sum();
        Node::Leaf(Arc::new(Block {
            chunks: chunks.into_boxed_slice(),
            bytes,
        }))
    }

    fn branch(children: VecDeque<Node>) -> Node {
        let mut sizes = VecDeque::with_capacity(FANOUT);
        let mut total = 0;
        let mut count = 0;
        for c in &children {
            let b = c.byte_len();
            total += b;
            count += c.chunk_len();
            sizes.push_back(b);
        }
        Node::Branch(Arc::new(Branch {
            children,
            sizes,
            total,
            count,
        }))
    }

    fn branch_from(children: Vec<Node>) -> Node {
        Node::branch(children.into())
    }

    fn branch2(a: Node, b: Node) -> Node {
        let mut v = new_children();
        v.push_back(a);
        v.push_back(b);
        Node::branch(v)
    }

    #[inline]
    fn byte_len(&self) -> usize {
        match self {
            Node::Leaf(b) => b.bytes,
            Node::Branch(b) => b.total,
        }
    }

    #[inline]
    fn chunk_len(&self) -> usize {
        match self {
            Node::Leaf(b) => b.chunks.len(),
            Node::Branch(b) => b.count,
        }
    }

    /// A fresh right-spine of `height` branches terminating in `leaf` (a node of height 0).
    fn spine(height: u32, leaf: Node) -> Node {
        if height == 0 {
            leaf
        } else {
            let mut v = new_children();
            v.push_back(Node::spine(height - 1, leaf));
            Node::branch(v)
        }
    }

    /// Returns the chunk at chunk-`index` within this subtree (`index < self.chunk_len()`), descending
    /// by the per-child cached chunk counts. O(FANOUT · height).
    fn get_chunk(&self, mut index: usize) -> &Bytes {
        match self {
            Node::Leaf(b) => &b.chunks[index],
            Node::Branch(b) => {
                for child in &b.children {
                    let c = child.chunk_len();
                    if index < c {
                        return child.get_chunk(index);
                    }
                    index -= c;
                }
                unreachable!("chunk index past branch")
            }
        }
    }

    /// Recursively visits every chunk in this subtree, in order.
    fn for_each_chunk(&self, f: &mut impl FnMut(&Bytes)) {
        match self {
            Node::Leaf(b) => {
                for c in b.chunks.iter() {
                    f(c);
                }
            }
            Node::Branch(b) => {
                for child in &b.children {
                    child.for_each_chunk(f);
                }
            }
        }
    }

    /// The byte at `offset` (`offset < self.byte_len()`). O(log₃₂) via the per-child size scan.
    fn byte_at(&self, offset: usize) -> u8 {
        match self {
            Node::Leaf(b) => {
                let mut offset = offset;
                for chunk in b.chunks.iter() {
                    if offset < chunk.len() {
                        return chunk[offset];
                    }
                    offset -= chunk.len();
                }
                unreachable!("offset past leaf block")
            }
            Node::Branch(b) => {
                let mut i = 0;
                let mut acc = 0;
                loop {
                    let s = b.sizes[i];
                    if offset < acc + s {
                        break;
                    }
                    acc += s;
                    i += 1;
                }
                b.children[i].byte_at(offset - acc)
            }
        }
    }

    /// Sets the byte at `offset` (< `byte_len`). FBIP: `Arc::make_mut` mutates a uniquely-owned node
    /// in place and path-copies only shared spine nodes. The leaf chunk is edited in place when
    /// unique; when it is a large *shared* chunk it is SPLIT (bounded copy-on-write, see
    /// [`crate::cow_edit`]) rather than copied whole, which can grow the leaf block and, on overflow,
    /// split the leaf and propagate the split up the spine. Byte totals are unchanged (the edit
    /// preserves length), so only chunk counts and the split boundary's cached sizes are fixed up.
    /// O(log₃₂) + at most one bounded chunk copy.
    fn set_byte(&mut self, offset: usize, value: u8) -> InsertResult {
        match self {
            Node::Leaf(arc) => {
                let block = Arc::make_mut(arc);
                let mut chunks = core::mem::take(&mut block.chunks).into_vec();
                let mut k = 0;
                let mut local = offset;
                while local >= chunks[k].len() {
                    local -= chunks[k].len();
                    k += 1;
                }
                let edit =
                    crate::cow_edit(core::mem::take(&mut chunks[k]), local, 1, |s| s[0] = value);
                match edit {
                    crate::CowEdit::InPlace(c) => {
                        chunks[k] = c; // byte total unchanged
                        block.chunks = chunks.into_boxed_slice();
                        InsertResult::none()
                    }
                    crate::CowEdit::Split {
                        prefix,
                        mid,
                        suffix,
                    } => {
                        let mut pieces = Vec::with_capacity(3);
                        pieces.extend(prefix);
                        pieces.push(mid);
                        pieces.extend(suffix);
                        let added = pieces.len() - 1;
                        chunks.splice(k..=k, pieces); // replace the taken placeholder slot
                        finish_leaf(block, chunks, added)
                    }
                }
            }
            Node::Branch(arc) => {
                let branch = Arc::make_mut(arc);
                let mut i = 0;
                let mut acc = 0;
                loop {
                    let s = branch.sizes[i];
                    if offset < acc + s {
                        break;
                    }
                    acc += s;
                    i += 1;
                }
                let res = branch.children[i].set_byte(offset - acc, value);
                absorb_child_insert(branch, i, res)
            }
        }
    }

    /// Overwrites `count` bytes starting at `offset` within this node with bytes streamed from
    /// `value` (`offset + count <= byte_len`). FBIP: `Arc::make_mut` mutates uniquely-owned nodes in
    /// place and path-copies only shared spine nodes; each covered leaf chunk is edited with the
    /// bounded [`crate::cow_edit`] — in place when uniquely owned, else a copy bounded by the edited
    /// span, sharing the untouched prefix/suffix of a large shared chunk rather than copying it whole.
    /// Byte totals are conserved, but a bounded-COW split of a boundary chunk GROWS the chunk count, so
    /// this threads a B-tree [`InsertResult`] back up the spine (like [`Node::set_byte`]): a leaf may
    /// split at `FANOUT` and a branch may split once after absorbing its children's inserts. Only the
    /// first- and last-covered child of a branch can split (interior children are fully covered, so
    /// every chunk is rewritten whole → `InPlace`, no new chunks), so a branch gains at most two
    /// children per overwrite — one `split_branch_off` suffices. O(log₃₂ + covered chunks).
    fn overwrite<R>(&mut self, offset: usize, count: usize, value: &mut R) -> InsertResult
    where
        R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
    {
        use etude_buffer::reader::Infallible as _;
        match self {
            Node::Leaf(arc) => {
                let block = Arc::make_mut(arc);
                let mut chunks = core::mem::take(&mut block.chunks).into_vec();
                let mut local = offset;
                let mut idx = 0;
                while local >= chunks[idx].len() {
                    local -= chunks[idx].len();
                    idx += 1;
                }
                let mut remaining = count;
                let mut added = 0;
                while remaining > 0 {
                    let here = (chunks[idx].len() - local).min(remaining);
                    let edit =
                        crate::cow_edit(core::mem::take(&mut chunks[idx]), local, here, |s| {
                            let mut dst: &mut [u8] = s;
                            value.infallible_copy_into(&mut dst);
                        });
                    let n = splice_cow_vec(&mut chunks, idx, edit);
                    added += n;
                    remaining -= here;
                    idx += 1 + n; // skip past any spliced-in prefix/suffix to the next original chunk
                    local = 0;
                }
                finish_leaf(block, chunks, added)
            }
            Node::Branch(arc) => {
                let branch = Arc::make_mut(arc);
                let mut local = offset;
                let mut i = 0;
                while local >= branch.sizes[i] {
                    local -= branch.sizes[i];
                    i += 1;
                }
                let mut remaining = count;
                let mut total_added = 0;
                while remaining > 0 {
                    // Byte totals are conserved, so the child's covered span is fixed by its ORIGINAL
                    // cached size even if it splits below.
                    let here = (branch.sizes[i] - local).min(remaining);
                    let res = branch.children[i].overwrite(local, here, value);
                    branch.count += res.added;
                    total_added += res.added;
                    remaining -= here;
                    local = 0;
                    let mut step = 1;
                    if let Some(sib) = res.overflow {
                        // The child split: fix its now-shrunk boundary size and splice the sibling in.
                        // Defer any split of THIS branch until the whole walk finishes so indices stay
                        // stable (only the first/last-covered child can reach here, ≤2 inserts total).
                        branch.sizes[i] = branch.children[i].byte_len();
                        let sib_bytes = sib.byte_len();
                        branch.children.insert(i + 1, sib);
                        branch.sizes.insert(i + 1, sib_bytes);
                        step = 2; // skip past the inserted sibling to the next original child
                    }
                    i += step;
                }
                if branch.children.len() > FANOUT {
                    InsertResult {
                        added: total_added,
                        overflow: Some(split_branch_off(branch)),
                    }
                } else {
                    InsertResult {
                        added: total_added,
                        overflow: None,
                    }
                }
            }
        }
    }

    /// Recursively validates node invariants, returning `(byte_len, chunk_count)`. `height` is the
    /// number of branch levels below/at this node (0 = leaf). Test-only.
    #[cfg(test)]
    fn check_invariants(&self, height: u32) -> (usize, usize) {
        match self {
            Node::Leaf(b) => {
                assert_eq!(height, 0, "leaf at non-zero height");
                assert!(
                    !b.chunks.is_empty() && b.chunks.len() <= crate::FANOUT,
                    "leaf block size {} out of 1..={}",
                    b.chunks.len(),
                    crate::FANOUT
                );
                let bytes: usize = b.chunks.iter().map(|c| c.len()).sum();
                assert!(
                    b.chunks.iter().all(|c| !c.is_empty()),
                    "empty chunk in leaf"
                );
                assert_eq!(bytes, b.bytes, "leaf cached bytes wrong");
                (bytes, b.chunks.len())
            }
            Node::Branch(b) => {
                assert_ne!(height, 0, "branch at height 0");
                assert!(
                    !b.children.is_empty() && b.children.len() <= crate::FANOUT,
                    "branch fanout {} out of 1..={}",
                    b.children.len(),
                    crate::FANOUT
                );
                assert_eq!(
                    b.children.len(),
                    b.sizes.len(),
                    "sizes not parallel to children"
                );
                let mut total = 0;
                let mut count = 0;
                for (child, &size) in b.children.iter().zip(b.sizes.iter()) {
                    let (cb, cc) = child.check_invariants(height - 1);
                    assert_eq!(cb, size, "branch size table entry wrong");
                    total += cb;
                    count += cc;
                }
                assert_eq!(total, b.total, "branch cached total wrong");
                assert_eq!(count, b.count, "branch cached count wrong");
                (total, count)
            }
        }
    }

    #[inline]
    fn is_empty_branch(&self) -> bool {
        matches!(self, Node::Branch(b) if b.children.is_empty())
    }
}

/// A relaxed-radix tree of chunk blocks. See the module docs.
#[derive(Clone, Default)]
pub(crate) struct Tree {
    root: Option<Node>,
    /// Branch levels above the leaves: 0 = `root` is a `Leaf`, 1 = a `Branch` of leaves, etc.
    height: u32,
    chunk_count: usize,
}

impl Tree {
    #[inline]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(crate) fn chunk_count(&self) -> usize {
        self.chunk_count
    }

    /// Visits every chunk in order via a simple recursive DFS — cheaper than the resumable [`Chunks`]
    /// iterator (no per-chunk stack save/restore), for hot bulk reads like flatten.
    pub(crate) fn for_each_chunk(&self, f: &mut impl FnMut(&Bytes)) {
        if let Some(root) = &self.root {
            root.for_each_chunk(f);
        }
    }

    /// Returns the chunk at chunk-`index` (0 = first) via an O(log₃₂) descent on the per-subtree chunk
    /// counts, or `None` if out of range.
    pub(crate) fn get_chunk(&self, index: usize) -> Option<&Bytes> {
        let root = self.root.as_ref()?;
        (index < self.chunk_count).then(|| root.get_chunk(index))
    }

    /// Validates the tree's structural invariants and cached sizes/counts against the actual nodes.
    /// Test-only.
    #[cfg(test)]
    pub(crate) fn check_invariants(&self) {
        match &self.root {
            Some(root) => {
                let (_, count) = root.check_invariants(self.height);
                assert_eq!(count, self.chunk_count, "tree cached chunk_count wrong");
            }
            None => assert_eq!(self.chunk_count, 0, "empty tree with non-zero chunk_count"),
        }
    }

    #[inline]
    pub(crate) fn byte_len(&self) -> usize {
        self.root.as_ref().map_or(0, Node::byte_len)
    }

    #[inline]
    pub(crate) fn byte_at(&self, offset: usize) -> u8 {
        self.root
            .as_ref()
            .expect("byte_at on an empty tree")
            .byte_at(offset)
    }

    pub(crate) fn set_byte(&mut self, offset: usize, value: u8) {
        let res = self
            .root
            .as_mut()
            .expect("set_byte on an empty tree")
            .set_byte(offset, value);
        self.chunk_count += res.added;
        if let Some(sib) = res.overflow {
            let old = self.root.take().unwrap();
            self.root = Some(Node::branch2(old, sib));
            self.height += 1;
        }
    }

    #[inline]
    pub(crate) fn overwrite<R>(&mut self, offset: usize, count: usize, value: &mut R)
    where
        R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
    {
        if count == 0 {
            return;
        }
        let res = self
            .root
            .as_mut()
            .expect("overwrite on an empty tree")
            .overwrite(offset, count, value);
        self.chunk_count += res.added;
        if let Some(sib) = res.overflow {
            let old = self.root.take().unwrap();
            self.root = Some(Node::branch2(old, sib));
            self.height += 1;
        }
    }

    /// Appends a block (`1..=FANOUT` chunks) at the back. In-place when uniquely owned.
    pub(crate) fn push_block(&mut self, chunks: Vec<Bytes>) {
        debug_assert!(!chunks.is_empty() && chunks.len() <= FANOUT);
        let lc = chunks.len();
        self.chunk_count += lc;
        let leaf = Node::leaf(chunks);
        let lb = leaf.byte_len();
        match self.root.take() {
            None => {
                self.height = 0;
                self.root = Some(leaf);
            }
            Some(root) if self.height == 0 => {
                self.height = 1;
                self.root = Some(Node::branch2(root, leaf));
            }
            Some(mut root) => {
                let mut slot = Some(leaf);
                match push_inplace(&mut root, self.height, &mut slot, lb, lc) {
                    Push::Placed => self.root = Some(root),
                    Push::Full => {
                        let sibling = Node::spine(self.height, slot.take().unwrap());
                        self.height += 1;
                        self.root = Some(Node::branch2(root, sibling));
                    }
                    Push::Shared => {
                        let leaf = slot.take().unwrap();
                        self.root = Some(match push_rightmost(&root, self.height, leaf.clone()) {
                            Some(updated) => updated,
                            None => {
                                let sibling = Node::spine(self.height, leaf);
                                self.height += 1;
                                Node::branch2(root, sibling)
                            }
                        });
                    }
                }
            }
        }
    }

    pub(crate) fn pop_front_block(&mut self) -> Option<Vec<Bytes>> {
        self.pop_end_block(End::Front)
    }

    pub(crate) fn pop_back_block(&mut self) -> Option<Vec<Bytes>> {
        self.pop_end_block(End::Back)
    }

    fn pop_end_block(&mut self, end: End) -> Option<Vec<Bytes>> {
        {
            let root = self.root.as_mut()?;
            if let Node::Leaf(_) = root {
                let block = into_block(self.root.take().unwrap());
                self.chunk_count -= block.len();
                self.height = 0;
                return Some(block);
            }
            if let Some(block) = pop_inplace(root, end) {
                self.chunk_count -= block.len();
                let empty = root.is_empty_branch();
                let collapse = matches!(root, Node::Branch(b) if b.children.len() == 1);
                if empty {
                    self.root = None;
                    self.height = 0;
                } else if collapse {
                    let taken = self.root.take();
                    self.install(taken);
                }
                return Some(block);
            }
        }
        // Persistent fallback (shared tree).
        let root = self.root.take()?;
        let (block, new_root) = match &root {
            Node::Leaf(_) => (into_block(root), None),
            Node::Branch(_) => pop_persistent(&root, self.height, end),
        };
        self.chunk_count -= block.len();
        self.install(new_root);
        Some(block)
    }

    /// Total byte length of the rightmost leaf block, or `None` if empty. O(log) rightmost descent.
    pub(crate) fn back_block_bytes(&self) -> Option<usize> {
        let mut node = self.root.as_ref()?;
        loop {
            match node {
                Node::Leaf(b) => return Some(b.bytes),
                Node::Branch(b) => node = b.children.back()?,
            }
        }
    }

    pub(crate) fn front_chunk_len(&self) -> Option<usize> {
        self.front_chunk().map(|c| c.len())
    }

    /// The leftmost chunk, or `None` if empty. O(log) leftmost descent.
    pub(crate) fn front_chunk(&self) -> Option<&Bytes> {
        let mut node = self.root.as_ref()?;
        loop {
            match node {
                Node::Leaf(b) => return b.chunks.first(),
                Node::Branch(b) => node = b.children.front()?,
            }
        }
    }

    pub(crate) fn chunks(&self) -> Chunks<'_> {
        let mut it = Chunks::empty();
        if let Some(root) = &self.root {
            it.descend(root);
        }
        it
    }

    /// Concatenates `left ++ right`, sharing all subtrees away from the seam. O(log₃₂).
    ///
    /// Consumes both operands by value: an empty operand is returned whole (no work), and the seam
    /// merge *moves* Arc handles out of uniquely-owned nodes rather than cloning them — so concat of
    /// owned trees (the common case: freshly split/built spines) does zero refcount churn away from
    /// genuinely shared subtrees.
    pub(crate) fn concat(left: Tree, right: Tree) -> Tree {
        if left.root.is_none() {
            return right;
        }
        if right.root.is_none() {
            return left;
        }
        let chunk_count = left.chunk_count + right.chunk_count;
        let (nodes, h) = merge(
            left.root.unwrap(),
            left.height,
            right.root.unwrap(),
            right.height,
        );
        let (root, height) = if nodes.len() == 1 {
            (nodes.into_iter().next().unwrap(), h)
        } else {
            (Node::branch_from(nodes), h + 1)
        };
        Tree {
            root: Some(root),
            height,
            chunk_count,
        }
    }

    /// Splits into `([0, offset), [offset, len))`, sharing subtrees on either side. O(log₃₂).
    pub(crate) fn split(&self, offset: usize) -> (Tree, Tree) {
        match &self.root {
            None => (Tree::new(), Tree::new()),
            Some(root) => {
                if offset == 0 {
                    return (Tree::new(), self.clone());
                }
                if offset >= self.byte_len() {
                    return (self.clone(), Tree::new());
                }
                let (l, r) = split_node(root, offset);
                (
                    Tree::from_root(l, self.height),
                    Tree::from_root(r, self.height),
                )
            }
        }
    }

    /// Extracts the sub-tree over the byte range `[start, end)` in a SINGLE descent, sharing every
    /// interior subtree (O(1) Arc clone) and recursing only into the ≤2 boundary children whose span
    /// straddles `start`/`end`. O(log₃₂). Unlike two composed [`Tree::split`]s this never materializes
    /// the discarded ends. `start == end` yields an empty tree; the full range clones the root.
    pub(crate) fn subrange(&self, start: usize, end: usize) -> Tree {
        debug_assert!(start <= end && end <= self.byte_len());
        match &self.root {
            None => Tree::new(),
            Some(root) => {
                if start == end {
                    return Tree::new();
                }
                if start == 0 && end == self.byte_len() {
                    return self.clone();
                }
                Tree::from_root(subrange_node(root, start, end), self.height)
            }
        }
    }

    /// Builds a tree from a (possibly over-tall / single-child-spined) root, normalizing height and
    /// recomputing `chunk_count`.
    fn from_root(root: Option<Node>, height: u32) -> Tree {
        let mut t = Tree {
            root,
            height,
            chunk_count: 0,
        };
        t.normalize();
        t
    }

    /// Collapses single-child branches (adjusting `height`), drops an emptied root, and refreshes
    /// `chunk_count` from the root.
    fn normalize(&mut self) {
        loop {
            match &self.root {
                Some(b) if b.is_empty_branch() => {
                    self.root = None;
                    break;
                }
                Some(Node::Branch(b)) if b.children.len() == 1 => {
                    self.root = Some(b.children[0].clone());
                    self.height -= 1;
                }
                _ => break,
            }
        }
        if self.root.is_none() {
            self.height = 0;
        }
        self.chunk_count = self.root.as_ref().map_or(0, Node::chunk_len);
    }

    /// Reinstalls a (possibly shrunken) root after an in-place pop: collapses single-child branches
    /// and drops an emptied root. (Does not touch `chunk_count`; the caller maintains it.)
    fn install(&mut self, mut root: Option<Node>) {
        loop {
            match &root {
                Some(b) if b.is_empty_branch() => {
                    root = None;
                    break;
                }
                Some(Node::Branch(b)) if b.children.len() == 1 => {
                    root = Some(b.children[0].clone());
                    self.height -= 1;
                }
                _ => break,
            }
        }
        if root.is_none() {
            self.height = 0;
        }
        self.root = root;
    }
}

#[derive(Clone, Copy)]
enum End {
    Front,
    Back,
}

fn push_inplace(
    node: &mut Node,
    height: u32,
    slot: &mut Option<Node>,
    lb: usize,
    lc: usize,
) -> Push {
    let Node::Branch(arc) = node else {
        return Push::Shared;
    };
    let Some(branch) = Arc::get_mut(arc) else {
        return Push::Shared;
    };
    if height == 1 {
        if branch.children.len() < FANOUT {
            branch.children.push_back(slot.take().unwrap());
            branch.sizes.push_back(lb);
            branch.total += lb;
            branch.count += lc;
            Push::Placed
        } else {
            Push::Full
        }
    } else {
        let last = branch.children.len() - 1;
        match push_inplace(&mut branch.children[last], height - 1, slot, lb, lc) {
            Push::Placed => {
                *branch.sizes.back_mut().unwrap() += lb;
                branch.total += lb;
                branch.count += lc;
                Push::Placed
            }
            Push::Full => {
                if branch.children.len() < FANOUT {
                    branch
                        .children
                        .push_back(Node::spine(height - 1, slot.take().unwrap()));
                    branch.sizes.push_back(lb);
                    branch.total += lb;
                    branch.count += lc;
                    Push::Placed
                } else {
                    Push::Full
                }
            }
            Push::Shared => Push::Shared,
        }
    }
}

fn pop_inplace(node: &mut Node, end: End) -> Option<Vec<Bytes>> {
    let Node::Branch(arc) = node else {
        return None;
    };
    let branch = Arc::get_mut(arc)?;
    let idx = match end {
        End::Front => 0,
        End::Back => branch.children.len() - 1,
    };
    match &branch.children[idx] {
        Node::Leaf(_) => {
            let (leaf, sz) = remove_at(branch, end);
            branch.total -= sz;
            let block = into_block(leaf);
            branch.count -= block.len();
            Some(block)
        }
        Node::Branch(_) => {
            let block = pop_inplace(&mut branch.children[idx], end)?;
            let lb: usize = block.iter().map(|c| c.len()).sum();
            if branch.children[idx].is_empty_branch() {
                remove_at(branch, end);
            } else {
                branch.sizes[idx] -= lb;
            }
            branch.total -= lb;
            branch.count -= block.len();
            Some(block)
        }
    }
}

#[inline]
fn remove_at(branch: &mut Branch, end: End) -> (Node, usize) {
    match end {
        End::Front => (
            branch.children.pop_front().unwrap(),
            branch.sizes.pop_front().unwrap(),
        ),
        End::Back => (
            branch.children.pop_back().unwrap(),
            branch.sizes.pop_back().unwrap(),
        ),
    }
}

#[inline]
fn into_block(node: Node) -> Vec<Bytes> {
    match node {
        Node::Leaf(arc) => match Arc::try_unwrap(arc) {
            Ok(block) => block.chunks.into_vec(),
            Err(arc) => arc.chunks.to_vec(),
        },
        Node::Branch(_) => unreachable!("into_block on a branch"),
    }
}

/// Moves a branch node's children out (draining the `VecDeque` when uniquely owned), cloning only if
/// the node is shared. Mirrors [`into_block`] at the branch level.
#[inline]
fn into_children(node: Node) -> VecDeque<Node> {
    match node {
        Node::Branch(arc) => match Arc::try_unwrap(arc) {
            Ok(branch) => branch.children,
            Err(arc) => arc.children.clone(),
        },
        Node::Leaf(_) => unreachable!("into_children on a leaf"),
    }
}

// --- concat -------------------------------------------------------------

/// Merges `left` (height `hl`) and `right` (height `hr`) along the seam, returning 1 or 2 nodes at
/// height `max(hl, hr)`. Only the seam path is repacked; all other children are *moved* out of
/// uniquely-owned nodes (cloned only when a subtree is genuinely shared).
fn merge(left: Node, hl: u32, right: Node, hr: u32) -> (Vec<Node>, u32) {
    if hl == 0 && hr == 0 {
        let mut chunks = into_block(left);
        chunks.extend(into_block(right));
        (repack_leaves(chunks), 0)
    } else if hl == hr {
        let mut lc = into_children(left);
        let mut rc = into_children(right);
        let last = lc.pop_back().unwrap();
        let first = rc.pop_front().unwrap();
        let (seam, _) = merge(last, hl - 1, first, hr - 1);
        let mut mids: Vec<Node> = lc.into();
        mids.extend(seam);
        mids.extend(rc);
        (repack_branches(mids), hl)
    } else if hl > hr {
        let mut lc = into_children(left);
        let last = lc.pop_back().unwrap();
        let (seam, _) = merge(last, hl - 1, right, hr);
        let mut mids: Vec<Node> = lc.into();
        mids.extend(seam);
        (repack_branches(mids), hl)
    } else {
        let mut rc = into_children(right);
        let first = rc.pop_front().unwrap();
        let (seam, _) = merge(left, hl, first, hr - 1);
        let mut mids: Vec<Node> = seam;
        mids.extend(rc);
        (repack_branches(mids), hr)
    }
}

/// Packs `chunks` (≤ 2·FANOUT) into 1 or 2 leaves of ≤ FANOUT chunks each.
fn repack_leaves(mut chunks: Vec<Bytes>) -> Vec<Node> {
    debug_assert!(chunks.len() <= 2 * FANOUT);
    if chunks.len() <= FANOUT {
        alloc::vec![Node::leaf(chunks)]
    } else {
        let rest = chunks.split_off(FANOUT);
        alloc::vec![Node::leaf(chunks), Node::leaf(rest)]
    }
}

/// Packs `children` (≤ 2·FANOUT nodes at some height `h-1`) into 1 or 2 branches at height `h`.
fn repack_branches(mut children: Vec<Node>) -> Vec<Node> {
    debug_assert!(children.len() <= 2 * FANOUT);
    if children.len() <= FANOUT {
        alloc::vec![Node::branch_from(children)]
    } else {
        let rest = children.split_off(FANOUT);
        alloc::vec![Node::branch_from(children), Node::branch_from(rest)]
    }
}

// --- split --------------------------------------------------------------

/// Splits `node` at byte `offset` into `(left, right)`, sharing every subtree that falls entirely on
/// one side. Only the single child straddling `offset` is recursively split. (Leaf vs branch is
/// determined by the node itself, so no height is threaded.)
fn split_node(node: &Node, offset: usize) -> (Option<Node>, Option<Node>) {
    if offset == 0 {
        return (None, Some(node.clone()));
    }
    if offset == node.byte_len() {
        return (Some(node.clone()), None);
    }
    match node {
        Node::Leaf(b) => {
            let mut rem = offset;
            let mut left = Vec::new();
            let mut right = Vec::new();
            for chunk in b.chunks.iter() {
                if rem == 0 {
                    right.push(chunk.clone());
                } else if rem >= chunk.len() {
                    rem -= chunk.len();
                    left.push(chunk.clone());
                } else {
                    let mut c = chunk.clone();
                    let l = c.split_to(rem); // l = [0, rem), c = [rem, ..)
                    left.push(l);
                    right.push(c);
                    rem = 0;
                }
            }
            let l = (!left.is_empty()).then(|| Node::leaf(left));
            let r = (!right.is_empty()).then(|| Node::leaf(right));
            (l, r)
        }
        Node::Branch(b) => {
            // find the child straddling `offset`
            let mut i = 0;
            let mut acc = 0;
            loop {
                let s = b.sizes[i];
                if offset < acc + s {
                    break;
                }
                acc += s;
                i += 1;
            }
            let (cl, cr) = split_node(&b.children[i], offset - acc);

            let mut lchildren: VecDeque<Node> = b.children.iter().take(i).cloned().collect();
            if let Some(c) = cl {
                lchildren.push_back(c);
            }
            let mut rchildren: VecDeque<Node> = VecDeque::new();
            if let Some(c) = cr {
                rchildren.push_back(c);
            }
            rchildren.extend(b.children.iter().skip(i + 1).cloned());

            let l = (!lchildren.is_empty()).then(|| Node::branch(lchildren));
            let r = (!rchildren.is_empty()).then(|| Node::branch(rchildren));
            (l, r)
        }
    }
}

/// Extracts `[start, end)` from `node` (`0 <= start < end <= node.byte_len()`), returning a node at
/// the SAME height as `node` (branches are always re-wrapped, so the caller's `from_root` normalizes
/// any single-child spine). Interior children that fall entirely inside the range are shared whole
/// (O(1) clone via the full-span fast path); only the boundary children recurse. `None` iff empty.
fn subrange_node(node: &Node, start: usize, end: usize) -> Option<Node> {
    if start == 0 && end == node.byte_len() {
        return Some(node.clone());
    }
    if start == end {
        return None;
    }
    match node {
        Node::Leaf(b) => {
            let mut out = Vec::new();
            let mut acc = 0;
            for chunk in b.chunks.iter() {
                let (cs, ce) = (acc, acc + chunk.len());
                acc = ce;
                if ce <= start {
                    continue;
                }
                if cs >= end {
                    break;
                }
                let lo = start.saturating_sub(cs);
                let hi = end.min(ce) - cs;
                out.push(chunk.slice(lo..hi)); // O(1) shared view
            }
            (!out.is_empty()).then(|| Node::leaf(out))
        }
        Node::Branch(b) => {
            let mut children = VecDeque::new();
            let mut acc = 0;
            for (i, child) in b.children.iter().enumerate() {
                let (cs, ce) = (acc, acc + b.sizes[i]);
                acc = ce;
                if ce <= start {
                    continue;
                }
                if cs >= end {
                    break;
                }
                if let Some(c) = subrange_node(child, start.saturating_sub(cs), end.min(ce) - cs) {
                    children.push_back(c);
                }
            }
            (!children.is_empty()).then(|| Node::branch(children))
        }
    }
}

// --- persistent (shared-tree) push/pop fallbacks ------------------------

fn push_rightmost(node: &Node, height: u32, leaf: Node) -> Option<Node> {
    let Node::Branch(b) = node else {
        unreachable!("push_rightmost below a leaf")
    };
    let children = &b.children;
    if height == 1 {
        if children.len() < FANOUT {
            let mut kids = children.clone();
            kids.push_back(leaf);
            Some(Node::branch(kids))
        } else {
            None
        }
    } else {
        let last = children.len() - 1;
        if let Some(updated) = push_rightmost(&children[last], height - 1, leaf.clone()) {
            let mut kids = children.clone();
            kids[last] = updated;
            Some(Node::branch(kids))
        } else if children.len() < FANOUT {
            let mut kids = children.clone();
            kids.push_back(Node::spine(height - 1, leaf));
            Some(Node::branch(kids))
        } else {
            None
        }
    }
}

fn pop_persistent(node: &Node, height: u32, end: End) -> (Vec<Bytes>, Option<Node>) {
    let Node::Branch(b) = node else {
        unreachable!("pop_persistent below a leaf")
    };
    let children = &b.children;
    let idx = match end {
        End::Front => 0,
        End::Back => children.len() - 1,
    };
    if height == 1 {
        let block = into_block(children[idx].clone());
        let mut kids = children.clone();
        remove_at_deque(&mut kids, end);
        let new = (!kids.is_empty()).then(|| Node::branch(kids));
        (block, new)
    } else {
        let (block, child) = pop_persistent(&children[idx], height - 1, end);
        let mut kids = children.clone();
        match child {
            Some(c) => kids[idx] = c,
            None => remove_at_deque(&mut kids, end),
        }
        let new = (!kids.is_empty()).then(|| Node::branch(kids));
        (block, new)
    }
}

#[inline]
fn remove_at_deque(kids: &mut VecDeque<Node>, end: End) {
    match end {
        End::Front => {
            kids.pop_front();
        }
        End::Back => {
            kids.pop_back();
        }
    }
}

/// Ordered iterator over a tree's `Bytes` chunks. See [`Tree::chunks`].
///
/// The descent stack is a `Vec`, but it only ever allocates for a *deep* tree — a shallow rope
/// iterates its `head`/deque directly and never builds this iterator — so the common case pays
/// nothing and the deep case amortizes one small allocation over the whole walk.
pub(crate) struct Chunks<'a> {
    stack: Vec<alloc::collections::vec_deque::Iter<'a, Node>>,
    leaf: core::slice::Iter<'a, Bytes>,
}

impl<'a> Chunks<'a> {
    fn empty() -> Self {
        Chunks {
            stack: Vec::new(),
            leaf: [].iter(),
        }
    }

    /// Walks the leftmost spine of `node`, pushing each branch's children iterator and leaving
    /// `self.leaf` on the leftmost leaf.
    fn descend(&mut self, node: &'a Node) {
        let mut node = node;
        loop {
            match node {
                Node::Leaf(b) => {
                    self.leaf = b.chunks.iter();
                    return;
                }
                Node::Branch(b) => {
                    let mut it = b.children.iter();
                    match it.next() {
                        Some(first) => {
                            self.stack.push(it);
                            node = first;
                        }
                        None => return,
                    }
                }
            }
        }
    }
}

impl<'a> Chunks<'a> {
    /// Cold path: the current leaf is exhausted — walk the branch stack to the next leaf.
    #[cold]
    fn next_leaf(&mut self) -> Option<&'a Bytes> {
        loop {
            let top = self.stack.last_mut()?;
            if let Some(node) = top.next() {
                self.descend(node);
                if let Some(chunk) = self.leaf.next() {
                    return Some(chunk);
                }
            } else {
                self.stack.pop();
            }
        }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a Bytes;

    /// Hot path is a single `leaf.next()`; the stack walk is out-of-line so it inlines tightly.
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self.leaf.next() {
            some @ Some(_) => some,
            None => self.next_leaf(),
        }
    }
}
