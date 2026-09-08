//! Pure scheduling decisions. Durable eligibility and execution have separate owners.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    VisibleRequired,
    NearbyRequired,
    VisibleOptional,
    SectionRequired,
    SectionOptional,
    Library,
}

#[derive(Default)]
pub struct Turns {
    local: usize,
}

impl Turns {
    pub fn order(&self) -> [Tier; 6] {
        use Tier::*;
        if self.local >= 8 {
            [
                VisibleRequired,
                NearbyRequired,
                Library,
                VisibleOptional,
                SectionRequired,
                SectionOptional,
            ]
        } else {
            [
                VisibleRequired,
                NearbyRequired,
                VisibleOptional,
                SectionRequired,
                SectionOptional,
                Library,
            ]
        }
    }

    pub fn completed(&mut self, tier: Tier) {
        if tier == Tier::Library {
            self.local = 0;
        } else {
            self.local = self.local.saturating_add(1);
        }
    }
}
