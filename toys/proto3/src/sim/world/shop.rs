//! Shopkeeper stock and buying. Pure over a `Catalog`: integration implements it for Content.items.
//! Honest keepers sell fair items at fair prices; deceivers sell trash or cursed items at a rare's price.
use crate::rng::mix;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemId(pub u16);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    /// Fair price in gold.
    fn base(self) -> u32 {
        match self {
            Rarity::Common => 20,
            Rarity::Uncommon => 50,
            Rarity::Rare => 120,
            Rarity::Epic => 300,
            Rarity::Legendary => 800,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemInfo {
    pub rarity: Rarity,
    /// carries at least one negative modifier
    pub cursed: bool,
}

/// What the shop needs from the item data.
pub trait Catalog {
    fn len(&self) -> usize;
    fn info(&self, id: ItemId) -> Option<ItemInfo>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Deal {
    pub item: ItemId,
    pub price: u32,
    pub honest: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuyErr {
    NotEnough { price: u32, have: u32 },
    NoSuchOffer,
    /// not a shop, or the hero is not at it
    Closed,
}

/// Offers per shop.
pub const OFFERS: usize = 4;
/// Fair price jitter, permille around the base: 900..=1100.
const JITTER_LO: u64 = 900;
const JITTER_SPAN: u64 = 201;
/// A deceiver prices trash like this rarity.
const SCAM_PRICED_AS: Rarity = Rarity::Rare;
/// ... times this, permille.
const SCAM_MARKUP: u64 = 1500;

fn fair(r: Rarity, h: u64) -> u32 {
    (u64::from(r.base()) * (JITTER_LO + h % JITTER_SPAN) / 1000) as u32
}

/// Item a deceiver may push: trash (low rarity) or cursed.
fn trash(i: ItemInfo) -> bool {
    i.cursed || i.rarity <= Rarity::Uncommon
}

/// Seeded stock: up to OFFERS distinct items (fewer when the catalog has none of the wanted kind).
pub fn stock(cat: &impl Catalog, honest: bool, key: u64) -> Vec<Deal> {
    let n = cat.len();
    let mut out: Vec<Deal> = Vec::with_capacity(OFFERS);
    if n == 0 {
        return out;
    }
    for k in 0..OFFERS as u64 {
        let h = mix(key ^ k);
        let start = (h % n as u64) as usize;
        let pick = (0..n).map(|d| (start + d) % n).find_map(|ix| {
            let id = ItemId(u16::try_from(ix).ok()?);
            let info = cat.info(id)?;
            let wanted = if honest { !info.cursed } else { trash(info) };
            (wanted && !out.iter().any(|d| d.item == id)).then_some((id, info))
        });
        if let Some((item, info)) = pick {
            let price = if honest { fair(info.rarity, h >> 8) } else { (u64::from(fair(SCAM_PRICED_AS, h >> 8)) * SCAM_MARKUP / 1000) as u32 };
            out.push(Deal { item, price, honest });
        }
    }
    out
}

/// Buy deal `ix` from `deals` with `gold`: sold deals are removed. Returns the item and the gold left.
pub fn buy(deals: &mut Vec<Deal>, ix: usize, gold: u32) -> Result<(ItemId, u32), BuyErr> {
    let d = *deals.get(ix).ok_or(BuyErr::NoSuchOffer)?;
    let left = gold.checked_sub(d.price).ok_or(BuyErr::NotEnough { price: d.price, have: gold })?;
    deals.remove(ix);
    Ok((d.item, left))
}

/// The item data is the catalog: an item is cursed when any of its mods is negative.
impl Catalog for crate::data::Content {
    fn len(&self) -> usize {
        self.items.len()
    }
    fn info(&self, id: ItemId) -> Option<ItemInfo> {
        use crate::data::Rarity as R;
        let it = self.items.get(usize::from(id.0))?;
        let rarity = match it.rarity {
            R::Common => Rarity::Common,
            R::Rare => Rarity::Rare,
            R::Epic => Rarity::Epic,
            R::Legendary | R::Unique => Rarity::Legendary,
        };
        Some(ItemInfo { rarity, cursed: it.mods.iter().any(|m| m.value < 0) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 12 items: rarity cycles Common..Legendary, every 5th is cursed.
    struct Cat;
    impl Catalog for Cat {
        fn len(&self) -> usize {
            12
        }
        fn info(&self, id: ItemId) -> Option<ItemInfo> {
            let i = usize::from(id.0);
            let rarity = [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::Epic, Rarity::Legendary][i % 5];
            (i < 12).then_some(ItemInfo { rarity, cursed: i % 5 == 4 })
        }
    }

    #[test]
    fn honest_stock_is_fair_and_never_cursed() {
        for key in 0..50 {
            let s = stock(&Cat, true, key);
            assert!(!s.is_empty() && s.len() <= OFFERS);
            for d in &s {
                let info = Cat.info(d.item).expect("known item");
                assert!(!info.cursed, "NEG: honest never sells cursed");
                let base = info.rarity.base();
                assert!((base * 9 / 10..=base * 11 / 10).contains(&d.price), "{d:?} base {base}");
                assert!(d.honest);
            }
            let ids: Vec<_> = s.iter().map(|d| d.item).collect();
            assert!(ids.iter().enumerate().all(|(i, a)| !ids[..i].contains(a)), "distinct items");
        }
    }

    #[test]
    fn deceiver_sells_trash_at_a_rares_price() {
        for key in 0..50 {
            for d in stock(&Cat, false, key) {
                let info = Cat.info(d.item).expect("known item");
                assert!(trash(info), "{d:?}");
                assert!(d.price >= Rarity::Rare.base() * 9 / 10 * 3 / 2, "overpriced: {d:?}");
                assert!(!d.honest);
            }
        }
        // NEG: deceiver stock differs from honest stock for the same key
        assert_ne!(stock(&Cat, true, 7), stock(&Cat, false, 7));
    }

    #[test]
    fn stock_is_deterministic_and_empty_catalog_is_empty() {
        assert_eq!(stock(&Cat, true, 3), stock(&Cat, true, 3));
        assert_ne!(stock(&Cat, true, 3), stock(&Cat, true, 4));
        struct Empty;
        impl Catalog for Empty {
            fn len(&self) -> usize {
                0
            }
            fn info(&self, _: ItemId) -> Option<ItemInfo> {
                None
            }
        }
        assert!(stock(&Empty, true, 1).is_empty());
    }

    #[test]
    fn buy_pays_removes_and_rejects() {
        let mut deals = vec![Deal { item: ItemId(1), price: 50, honest: true }, Deal { item: ItemId(2), price: 90, honest: true }];
        assert_eq!(buy(&mut deals, 0, 80), Ok((ItemId(1), 30)));
        assert_eq!(deals.len(), 1);
        // NEG: too poor leaves the stock alone and names both numbers
        assert_eq!(buy(&mut deals, 0, 89), Err(BuyErr::NotEnough { price: 90, have: 89 }));
        assert_eq!(deals.len(), 1);
        // NEG: bad index
        assert_eq!(buy(&mut deals, 5, 1000), Err(BuyErr::NoSuchOffer));
        assert_eq!(buy(&mut deals, 0, 90), Ok((ItemId(2), 0)));
        assert_eq!(buy(&mut deals, 0, 1000), Err(BuyErr::NoSuchOffer), "sold out");
    }
}
