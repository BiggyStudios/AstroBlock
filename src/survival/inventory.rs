//! The player's 36-slot inventory (9 hotbar + 27 main) and Minecraft's slot click rules.

use crate::survival::items::{Item, ItemStack, ToolKind};

pub const HOTBAR: usize = 9;
pub const SLOTS: usize = 36;

pub type Slot = Option<ItemStack>;

pub struct Inventory {
    /// 0..9 is the hotbar, 9..36 the main inventory.
    pub slots: [Slot; SLOTS],
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            slots: [None; SLOTS],
        }
    }
}

impl Inventory {
    /// Adds a stack like Minecraft's pickup: into matching stacks first, then empty slots,
    /// hotbar before the main inventory. Returns whatever didn't fit.
    pub fn add(&mut self, stack: ItemStack) -> Option<ItemStack> {
        let order: Vec<usize> = (0..SLOTS).collect();
        insert(&mut self.slots, stack, &order)
    }

    /// Sorts the main inventory (the hotbar stays as you arranged it): merges partial
    /// stacks, then orders tools, blocks and other items.
    pub fn sort(&mut self) {
        let mut stacks: Vec<ItemStack> = Vec::new();
        for slot in &mut self.slots[HOTBAR..] {
            let Some(stack) = slot.take() else { continue };
            let mut rest = Some(stack);
            for existing in stacks.iter_mut().filter(|s| s.stacks_with(&stack)) {
                let Some(r) = rest else { break };
                rest = merge(existing, r);
            }
            if let Some(r) = rest {
                stacks.push(r);
            }
        }
        stacks.sort_by_key(|s| (category(s.item), s.item, s.wear, std::cmp::Reverse(s.count)));
        for (slot, stack) in self.slots[HOTBAR..].iter_mut().zip(stacks) {
            *slot = Some(stack);
        }
    }

    /// Removes one item from a slot.
    pub fn take_one(&mut self, slot: usize) -> Option<ItemStack> {
        take(&mut self.slots[slot], 1)
    }
}

fn category(item: Item) -> u8 {
    match item {
        Item::Tool(ToolKind::Sword, _) => 0,
        Item::Tool(..) => 1,
        Item::Block(_) => 2,
        _ => 3,
    }
}

/// Moves as much of `stack` into `existing` as fits; returns the rest.
pub fn merge(existing: &mut ItemStack, stack: ItemStack) -> Option<ItemStack> {
    if !existing.stacks_with(&stack) {
        return Some(stack);
    }
    let room = existing.item.max_stack().saturating_sub(existing.count);
    let moved = room.min(stack.count);
    existing.count += moved;
    let left = stack.count - moved;
    (left > 0).then_some(ItemStack {
        count: left,
        ..stack
    })
}

/// Puts `stack` into `slots` (visited in `order`): matching stacks first, then empty slots.
/// Returns whatever didn't fit.
pub fn insert(slots: &mut [Slot], stack: ItemStack, order: &[usize]) -> Option<ItemStack> {
    let mut rest = Some(stack);
    for &i in order {
        let r = rest?;
        if let Some(existing) = &mut slots[i] {
            rest = merge(existing, r);
        }
    }
    for &i in order {
        let r = rest?;
        if slots[i].is_none() {
            slots[i] = Some(r);
            rest = None;
        }
    }
    rest
}

/// Takes up to `n` items out of a slot.
pub fn take(slot: &mut Slot, n: u8) -> Option<ItemStack> {
    let stack = slot.as_mut()?;
    let n = n.min(stack.count);
    stack.count -= n;
    let taken = ItemStack { count: n, ..*stack };
    if stack.count == 0 {
        *slot = None;
    }
    (n > 0).then_some(taken)
}

/// A mouse click on a slot while holding `cursor`, by Minecraft's rules:
/// * left: pick up the stack / put the held stack down / merge into it / swap them
/// * right: pick up half / put one item down
pub fn click(slot: &mut Slot, cursor: &mut Slot, right: bool) {
    match (*slot, *cursor) {
        (None, None) => {}
        (Some(stack), None) => {
            *cursor = if right {
                take(slot, stack.count.div_ceil(2))
            } else {
                slot.take()
            };
        }
        (None, Some(_)) => {
            *slot = if right {
                take(cursor, 1)
            } else {
                cursor.take()
            };
        }
        (Some(mut stack), Some(held)) => {
            if !stack.stacks_with(&held) {
                std::mem::swap(slot, cursor);
            } else if right {
                if stack.count < stack.item.max_stack() {
                    stack.count += 1;
                    *slot = Some(stack);
                    take(cursor, 1);
                }
            } else {
                *cursor = merge(&mut stack, held);
                *slot = Some(stack);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survival::items::Tier;
    use crate::world::block::Block;

    fn dirt(n: u8) -> ItemStack {
        ItemStack::new(Item::Block(Block::Dirt), n)
    }

    #[test]
    fn adding_fills_stacks_then_hotbar_first() {
        let mut inv = Inventory::default();
        assert_eq!(inv.add(dirt(40)), None);
        assert_eq!(inv.add(dirt(40)), None);
        assert_eq!(inv.slots[0], Some(dirt(64)));
        assert_eq!(inv.slots[1], Some(dirt(16)));
        let pick = ItemStack::one(Item::Tool(ToolKind::Pickaxe, Tier::Iron));
        assert_eq!(inv.add(pick), None);
        assert_eq!(inv.slots[2], Some(pick));
        // Full inventory: the rest comes back.
        for _ in 0..40 {
            inv.add(dirt(64));
        }
        assert_eq!(inv.add(dirt(5)), Some(dirt(5)));
    }

    #[test]
    fn sorting_merges_and_orders_the_main_inventory() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(dirt(3)); // hotbar: untouched
        inv.slots[12] = Some(dirt(30));
        inv.slots[20] = Some(ItemStack::new(Item::Stick, 5));
        inv.slots[30] = Some(dirt(50));
        inv.slots[35] = Some(ItemStack::one(Item::Tool(ToolKind::Sword, Tier::Stone)));
        inv.sort();
        assert_eq!(inv.slots[0], Some(dirt(3)));
        assert_eq!(
            inv.slots[9].map(|s| s.item),
            Some(Item::Tool(ToolKind::Sword, Tier::Stone))
        );
        assert_eq!(inv.slots[10], Some(dirt(64)));
        assert_eq!(inv.slots[11], Some(dirt(16)));
        assert_eq!(inv.slots[12], Some(ItemStack::new(Item::Stick, 5)));
        assert!(inv.slots[13..].iter().all(Option::is_none));
    }

    #[test]
    fn clicks_follow_minecraft() {
        let (mut slot, mut cursor) = (Some(dirt(9)), None);
        click(&mut slot, &mut cursor, true); // right: pick up half (rounded up)
        assert_eq!((slot, cursor), (Some(dirt(4)), Some(dirt(5))));
        click(&mut slot, &mut cursor, true); // right with a stack: put one down
        assert_eq!((slot, cursor), (Some(dirt(5)), Some(dirt(4))));
        click(&mut slot, &mut cursor, false); // left: merge all
        assert_eq!((slot, cursor), (Some(dirt(9)), None));
        let mut stick = Some(ItemStack::new(Item::Stick, 2));
        click(&mut slot, &mut stick, false); // different items swap
        assert_eq!(
            (slot, stick),
            (Some(ItemStack::new(Item::Stick, 2)), Some(dirt(9)))
        );
    }
}
