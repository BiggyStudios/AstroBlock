//! Furnaces: fuel burns down while an item smelts, 10 s per item, as in Minecraft. They keep
//! working when nobody is looking (and follow time warp).

use crate::survival::{
    inventory::Slot,
    items::{Item, ItemStack},
};

/// Seconds to smelt one item.
pub const COOK_TIME: f32 = 10.0;

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Furnace {
    pub input: Slot,
    pub fuel: Slot,
    pub output: Slot,
    /// Seconds of fuel left, and how long the current fuel item lasts in total.
    pub burn_left: f32,
    pub burn_total: f32,
    /// Progress on the current item (s).
    pub cook: f32,
}

impl Furnace {
    pub fn lit(&self) -> bool {
        self.burn_left > 0.0
    }

    /// What the input would smelt into, if the output slot can take it.
    fn product(&self) -> Option<Item> {
        let result = self.input?.item.smelted()?;
        match self.output {
            None => Some(result),
            Some(out) if out.item == result && out.count < result.max_stack() => Some(result),
            Some(_) => None,
        }
    }

    /// Advances by `dt` seconds (any length: it steps from event to event).
    pub fn update(&mut self, mut dt: f32) {
        while dt > 0.0 {
            let product = self.product();
            if !self.lit() {
                // Light the next fuel item, but only if there's something to smelt.
                let fuel = self.fuel.and_then(|f| f.item.fuel());
                match (product, fuel) {
                    (Some(_), Some(seconds)) => {
                        crate::survival::inventory::take(&mut self.fuel, 1);
                        self.burn_left = seconds;
                        self.burn_total = seconds;
                    }
                    _ => {
                        // Out of fuel: the item cools back down, as in Minecraft.
                        self.cook = (self.cook - 2.0 * dt).max(0.0);
                        return;
                    }
                }
            }
            let Some(result) = product else {
                // Burning with nothing to smelt.
                self.cook = 0.0;
                self.burn_left = (self.burn_left - dt).max(0.0);
                return;
            };
            let step = dt.min(self.burn_left).min(COOK_TIME - self.cook);
            self.burn_left -= step;
            self.cook += step;
            dt -= step;
            if self.cook >= COOK_TIME - 1e-4 {
                self.cook = 0.0;
                crate::survival::inventory::take(&mut self.input, 1);
                match &mut self.output {
                    Some(out) => out.count += 1,
                    None => self.output = Some(ItemStack::one(result)),
                }
            }
            if self.burn_left <= 1e-4 {
                self.burn_left = 0.0;
            }
        }
    }

    /// Everything inside (when the furnace is broken).
    pub fn contents(&self) -> impl Iterator<Item = ItemStack> {
        [self.input, self.fuel, self.output].into_iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::Block;

    #[test]
    fn smelts_with_fuel() {
        let mut f = Furnace {
            input: Some(ItemStack::new(Item::RawIron, 10)),
            fuel: Some(ItemStack::new(Item::Block(Block::OakPlanks), 1)),
            ..Default::default()
        };
        // One plank burns 15 s: one ingot, then half of the next, which cools off.
        f.update(5.0);
        assert!(f.lit() && f.output.is_none());
        f.update(30.0);
        assert_eq!(f.output, Some(ItemStack::new(Item::IronIngot, 1)));
        assert_eq!(f.input.unwrap().count, 9);
        assert!(!f.lit());
        // Coal smelts 8 items, even in one big (time-warped) step.
        f.fuel = Some(ItemStack::new(Item::Coal, 1));
        f.update(10_000.0);
        assert_eq!(f.output.unwrap().count, 9);
        assert_eq!(f.input.unwrap().count, 1);
        assert_eq!(f.fuel, None);
    }

    #[test]
    fn fuel_is_kept_until_there_is_something_to_smelt() {
        let mut f = Furnace {
            fuel: Some(ItemStack::new(Item::Coal, 3)),
            input: Some(ItemStack::new(Item::Stick, 3)), // not smeltable
            ..Default::default()
        };
        f.update(100.0);
        assert_eq!(f.fuel.unwrap().count, 3);
        assert!(!f.lit());
    }
}
