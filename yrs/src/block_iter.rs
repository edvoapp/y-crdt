use crate::block::{Block, BlockPtr, Item, ItemContent};
use crate::moving::{Move, RelativePosition};
use crate::types::array::ArraySliceConcat;
use crate::types::{BranchPtr, TypePtr, Value};
use crate::{Transaction, ID};
use std::ops::DerefMut;

#[derive(Debug, Clone)]
pub(crate) struct BlockIter {
    branch: BranchPtr,
    index: u32,
    rel: u32,
    next_item: Option<BlockPtr>,
    curr_move: Option<BlockPtr>,
    curr_move_start: Option<BlockPtr>,
    curr_move_end: Option<BlockPtr>,
    moved_stack: Vec<StackItem>,
    reached_end: bool,
}

impl BlockIter {
    pub fn new(branch: BranchPtr) -> Self {
        let next_item = branch.start;
        let reached_end = branch.start.is_none();
        BlockIter {
            branch,
            next_item,
            reached_end,
            curr_move: None,
            curr_move_start: None,
            curr_move_end: None,
            index: 0,
            rel: 0,
            moved_stack: Vec::default(),
        }
    }

    #[inline]
    pub fn rel(&self) -> u32 {
        self.rel
    }

    #[inline]
    pub fn finished(&self) -> bool {
        self.reached_end
    }

    #[inline]
    pub fn next_item(&self) -> Option<BlockPtr> {
        self.next_item
    }

    pub fn left(&self) -> Option<BlockPtr> {
        if self.reached_end {
            self.next_item
        } else if let Some(Block::Item(item)) = self.next_item.as_deref() {
            item.left
        } else {
            None
        }
    }

    pub fn right(&self) -> Option<BlockPtr> {
        if self.reached_end {
            None
        } else {
            self.next_item
        }
    }

    pub fn move_to(&mut self, index: u32, txn: &mut Transaction) {
        if index > self.index {
            self.forward(txn, index - self.index)
        } else if index < self.index {
            self.backward(txn, self.index - index)
        }
    }

    fn can_forward(&self, ptr: Option<BlockPtr>, len: u32) -> bool {
        if !self.reached_end || self.curr_move.is_some() {
            if len > 0 {
                return true;
            } else if let Some(Block::Item(item)) = ptr.as_deref() {
                return !item.is_countable()
                    || item.is_deleted()
                    || ptr == self.curr_move_end
                    || (self.reached_end && self.curr_move_end.is_none())
                    || item.moved != self.curr_move;
            }
        }

        false
    }

    pub fn forward(&mut self, txn: &mut Transaction, mut len: u32) {
        if len == 0 && self.next_item.is_none() {
            return;
        }

        if self.index + len > self.branch.content_len() || self.next_item.is_none() {
            panic!("Defect: length exceeded");
        }

        let mut item = self.next_item;
        self.index += len;
        if self.rel != 0 {
            len += self.rel;
            self.rel = 0;
        }

        let encoding = txn.store().options.offset_kind;
        while self.can_forward(item, len) {
            if item == self.curr_move_end
                || (self.reached_end && self.curr_move_end.is_none() && self.curr_move.is_some())
            {
                item = self.curr_move; // we iterate to the right after the current condition
                self.pop(txn);
            } else if item.is_none() {
                panic!("Defect: unexpected case during block iter forward");
            } else if let Some(Block::Item(i)) = item.as_deref() {
                if i.is_countable() && !i.is_deleted() && i.moved == self.curr_move && len > 0 {
                    let item_len = i.content_len(encoding);
                    if item_len > len {
                        self.rel = len;
                        len = 0;
                        break;
                    } else {
                        len -= item_len;
                    }
                } else if let ItemContent::Move(m) = &i.content {
                    if i.moved == self.curr_move {
                        if let Some(ptr) = self.curr_move {
                            self.moved_stack.push(StackItem::new(
                                self.curr_move_start,
                                self.curr_move_end,
                                ptr,
                            ));
                        }

                        let (start, end) = m.get_moved_coords(txn);
                        self.curr_move = item;
                        self.curr_move_start = start;
                        self.curr_move_end = end;
                        item = start;
                        continue;
                    }
                }

                if self.reached_end {
                    panic!("Defect: unexpected case during block iter forward");
                }

                if i.right.is_some() {
                    item = i.right;
                } else {
                    self.reached_end = true; //TODO: we need to ensure to iterate further if this.currMoveEnd === null
                }
            }
        }

        self.index -= len;
        self.next_item = item;
    }

    fn reduce_moves(&mut self, txn: &mut Transaction) {
        let mut item = self.next_item;
        if item.is_some() {
            while item == self.curr_move_start {
                item = self.curr_move;
                self.pop(txn);
            }
            self.next_item = item;
        }
    }

    pub fn backward(&mut self, txn: &mut Transaction, mut len: u32) {
        if self.index < len {
            panic!("Length exceeded");
        }
        self.index -= len;
        let encoding = txn.store().options.offset_kind;
        if self.reached_end {
            if let Some(Block::Item(next_item)) = self.next_item.as_deref() {
                self.rel = if next_item.is_countable() && !next_item.is_deleted() {
                    next_item.content_len(encoding)
                } else {
                    0
                };
            }
        }
        if self.rel >= len {
            self.rel -= len;
            return;
        }
        let mut item = self.next_item;
        if let Some(Block::Item(i)) = item.as_deref() {
            if let ItemContent::Move(_) = &i.content {
                item = i.left;
            } else {
                len += if i.is_countable() && !i.is_deleted() && i.moved == self.curr_move {
                    i.content_len(encoding)
                } else {
                    0
                };
                len -= self.rel;
            }
        }
        self.rel = 0;
        while let Some(Block::Item(i)) = item.as_deref() {
            if len == 0 {
                break;
            }

            if i.is_countable() && !i.is_deleted() && i.moved == self.curr_move {
                let item_len = i.content_len(encoding);
                if len < item_len {
                    self.rel = item_len - len;
                    len = 0;
                } else {
                    len -= item_len;
                }
                if len == 0 {
                    break;
                }
            } else if let ItemContent::Move(m) = &i.content {
                if i.moved == self.curr_move {
                    if let Some(curr_move) = self.curr_move {
                        self.moved_stack.push(StackItem::new(
                            self.curr_move_start,
                            self.curr_move_end,
                            curr_move,
                        ));
                    }
                    let (start, end) = m.get_moved_coords(txn);
                    self.curr_move = item;
                    self.curr_move_start = start;
                    self.curr_move_end = end;
                    item = start;
                    continue;
                }
            }

            if item == self.curr_move_start {
                item = self.curr_move; // we iterate to the left after the current condition
                self.pop(txn);
            }

            item = if let Some(Block::Item(i)) = item.as_deref() {
                i.left
            } else {
                None
            };
        }
        self.next_item = item;
    }

    /// We keep the moved-stack across several transactions. Local or remote changes can invalidate
    /// "moved coords" on the moved-stack.
    ///
    /// The reason for this is that if assoc < 0, then getMovedCoords will return the target.right
    /// item. While the computed item is on the stack, it is possible that a user inserts something
    /// between target and the item on the stack. Then we expect that the newly inserted item
    /// is supposed to be on the new computed item.
    fn pop(&mut self, txn: &mut Transaction) {
        let mut start = None;
        let mut end = None;
        let mut moved = None;
        if let Some(stack_item) = self.moved_stack.pop() {
            moved = Some(stack_item.moved_to);
            start = stack_item.start;
            end = stack_item.end;

            let moved_item = stack_item.moved_to.as_item().unwrap();
            if let ItemContent::Move(m) = &moved_item.content {
                if m.start.assoc && (m.start.within_range(start)) || (m.end.within_range(end)) {
                    let (s, e) = m.get_moved_coords(txn);
                    start = s;
                    end = e;
                }
            }
        }
        self.curr_move = moved;
        self.curr_move_start = start;
        self.curr_move_end = end;
        self.reached_end = false;
    }

    pub fn delete(&mut self, txn: &mut Transaction, mut len: u32) {
        let mut item = self.next_item;
        if self.index + len > self.branch.content_len() {
            panic!("Length exceeded");
        }

        let encoding = txn.store().options.offset_kind;
        let mut i: &Item;
        while len > 0 {
            while let Some(Block::Item(block)) = item.as_deref() {
                i = block;
                if !i.is_deleted()
                    && i.is_countable()
                    && !self.reached_end
                    && len > 0
                    && i.moved == self.curr_move
                    && item != self.curr_move_end
                {
                    if self.rel > 0 {
                        let mut id = i.id.clone();
                        id.clock += self.rel;
                        item = txn.store_mut().blocks.get_item_clean_start(&id);
                        i = if let Some(Block::Item(block)) = item.as_deref() {
                            block
                        } else {
                            panic!("Defect: should not happen")
                        };
                        self.rel = 0;
                    }
                    if len < i.content_len(encoding) {
                        let mut id = i.id.clone();
                        id.clock += len;
                        txn.store_mut().blocks.get_item_clean_start(&id);
                    }
                    len -= i.content_len(encoding);
                    txn.delete(item.unwrap());
                    if i.right.is_some() {
                        item = i.right;
                    } else {
                        self.reached_end = true;
                    }
                } else {
                    break;
                }
            }
            if len > 0 {
                self.next_item = item;
                self.forward(txn, 0);
                item = self.next_item;
            }
        }
        self.next_item = item;
    }

    fn slice<T>(&mut self, txn: &mut Transaction, mut len: u32, mut value: Vec<Value>) -> Vec<Value>
    where
        T: SliceConcat,
    {
        if self.index + len == self.branch.content_len() {
            panic!("Length exceeded")
        }
        self.index += len;
        let mut next_item = self.next_item;
        let encoding = txn.store().options.offset_kind;
        while len > 0 && !self.reached_end {
            while let Some(mut ptr) = next_item {
                if Some(ptr) != self.curr_move_end
                    && ptr.is_countable()
                    && !self.reached_end
                    && len > 0
                {
                    if let Block::Item(item) = ptr.deref_mut() {
                        if !item.is_deleted() && item.moved == self.curr_move {
                            let sliced_content =
                                T::slice(&mut item.content, self.rel as usize, len as usize);
                            let sliced_content_len = sliced_content.len() as u32;
                            len -= sliced_content_len;
                            value = T::concat(value, sliced_content);
                            if self.rel + sliced_content_len == item.content_len(encoding) {
                                self.rel = 0;
                            } else {
                                self.rel += sliced_content_len;
                                continue; // do not iterate to item.right
                            }
                        }

                        if item.right.is_some() {
                            next_item = item.right;
                            self.next_item = next_item;
                        } else {
                            self.reached_end = true;
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            if (!self.reached_end || self.curr_move.is_none()) && len > 0 {
                // always set nextItem before any method call
                self.next_item = next_item;
                self.forward(txn, 0);
                if self.next_item.is_none() {
                    panic!("Defect: block iterator slice has no next item")
                }
                next_item = self.next_item;
            }
        }
        self.next_item = next_item;
        if len < 0 {
            self.index -= len;
        }
        value
    }

    fn split_rel(&mut self, txn: &mut Transaction) {
        if self.rel > 0 {
            if let Some(ptr) = self.next_item {
                let mut item_id = ptr.id().clone();
                item_id.clock += self.rel;
                self.next_item = txn.store_mut().blocks.get_item_clean_start(&item_id);
                self.rel = 0;
            }
        }
    }

    pub fn insert_contents<I>(&mut self, txn: &mut Transaction, contents: I)
    where
        I: IntoIterator<Item = ItemContent>,
    {
        self.reduce_moves(txn);
        self.split_rel(txn);
        let parent = TypePtr::Branch(self.branch);
        let right = self.right();
        let mut left = self.left();
        for c in contents.into_iter() {
            let item_id = {
                let store = txn.store();
                let client = store.options.client_id;
                let clock = store.get_local_state();
                ID::new(client, clock)
            };
            let origin = left.map(|ptr| ptr.last_id());
            let right_origin = right.map(|ptr| ptr.id().clone());
            let mut block = Item::new(
                item_id,
                left,
                origin,
                right,
                right_origin,
                parent.clone(),
                None,
                c,
            );
            let mut ptr = BlockPtr::from(&mut block);
            ptr.integrate(txn, 0);
            left = Some(ptr);

            let store = txn.store_mut();
            let own_client_id = store.options.client_id;
            let local_block_list = store.blocks.get_client_blocks_mut(own_client_id);
            local_block_list.push(block);
        }

        if let Some(Block::Item(item)) = right.as_deref() {
            self.next_item = item.right;
        } else {
            self.next_item = left;
            self.reached_end = true;
        }
    }

    pub fn insert_move(
        &mut self,
        txn: &mut Transaction,
        start: RelativePosition,
        end: RelativePosition,
    ) {
        let content = ItemContent::Move(Box::new(Move::new(start, end, -1)));
        self.insert_contents(txn, [content]);
    }

    pub fn values<'a, 'txn>(&'a mut self, txn: &'txn mut Transaction) -> Values<'a, 'txn> {
        Values::new(self, txn)
    }
}

impl Iterator for BlockIter {
    type Item = Value;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

pub struct Values<'a, 'txn> {
    iter: &'a mut BlockIter,
    txn: &'txn mut Transaction,
}

impl<'a, 'txn> Values<'a, 'txn> {
    fn new(iter: &'a mut BlockIter, txn: &'txn mut Transaction) -> Self {
        Values { iter, txn }
    }
}

impl<'a, 'txn> Iterator for Values<'a, 'txn> {
    type Item = Value;

    fn next(&mut self) -> Option<Self::Item> {
        if self.iter.reached_end || self.iter.index == self.iter.branch.content_len() {
            None
        } else {
            let mut content = self
                .iter
                .slice::<ArraySliceConcat>(self.txn, 1, Vec::default());
            content.pop()
        }
    }
}

#[derive(Debug, Clone)]
struct StackItem {
    start: Option<BlockPtr>,
    end: Option<BlockPtr>,
    moved_to: BlockPtr,
}

impl StackItem {
    fn new(start: Option<BlockPtr>, end: Option<BlockPtr>, moved_to: BlockPtr) -> Self {
        StackItem {
            start,
            end,
            moved_to,
        }
    }
}

pub(crate) trait SliceConcat {
    fn slice(content: &mut ItemContent, offset: usize, len: usize) -> Vec<Value>;
    fn concat(a: Vec<Value>, b: Vec<Value>) -> Vec<Value>;
}