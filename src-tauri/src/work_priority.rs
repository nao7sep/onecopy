//! Pure scheduling decisions. Durable eligibility and execution have separate owners.

/// The two automatic workers. Preview preparation runs beside the heavy
/// lane shared by snapshots, similarity, face scoring, and transcription.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Previews,
    Heavy,
}

impl Lane {
    pub const ALL: [Lane; 2] = [Lane::Previews, Lane::Heavy];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Each lane's work for one tier. The heavy lane has no nearby tier:
    /// optional enrichment reaches past the visible region only through the
    /// section sweep.
    pub fn runs(self, tier: Tier) -> bool {
        !(self == Lane::Heavy && tier == Tier::Nearby)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Visible,
    Nearby,
    Section,
    Library,
}

/// One lane's fair turns: the selected, visible, and nearby tiers always come
/// first; after a bounded run of local turns, the library comes before the
/// section so neither starves.
#[derive(Default)]
pub struct Turns {
    local: usize,
}

impl Turns {
    pub fn order(&self) -> [Tier; 4] {
        use Tier::*;
        if self.local >= 8 {
            [Visible, Nearby, Library, Section]
        } else {
            [Visible, Nearby, Section, Library]
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
