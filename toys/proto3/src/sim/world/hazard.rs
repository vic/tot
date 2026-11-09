//! Mid-boss hazards: floor traps dropped by a living boss. Pure state; Sim glue (boss position, hero damage) is in world.rs.

/// Ticks a trap needs to arm (the view shows it dim until then), and how long it lasts in total.
pub const ARM_S: u32 = 1;
pub const LIFE_S: u32 = 20;
/// Trigger radius (mm) and damage in permille of max HP.
pub const TRIGGER_R: i32 = 1200;
pub const DMG: i32 = 150;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Trap {
    pub x: i32,
    pub y: i32,
    pub age: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrapState {
    /// dropped, harmless (the tell)
    Arming,
    Armed,
    Expired,
}

impl Trap {
    pub fn state(self, hz: u32) -> TrapState {
        if self.age >= hz * LIFE_S {
            TrapState::Expired
        } else if self.age >= hz * ARM_S {
            TrapState::Armed
        } else {
            TrapState::Arming
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trap_arms_then_expires() {
        let t = |age| Trap { x: 0, y: 0, age }.state(30);
        assert_eq!(t(0), TrapState::Arming);
        assert_eq!(t(29), TrapState::Arming);
        assert_eq!(t(30), TrapState::Armed);
        assert_eq!(t(599), TrapState::Armed);
        assert_eq!(t(600), TrapState::Expired);
    }
}
