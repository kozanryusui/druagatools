#![allow(
    dead_code,
    reason = "the server does not expose all confirmed Station event options yet"
)]

use std::num::{NonZeroU16, NonZeroU32};
use std::ops::BitOr;

use binrw::{BinRead, BinWrite};

use crate::protocol::tower::{PartyQuestId, SpecialQuestId};

use super::StationProtocolError;

/// Nonzero item identifier used by item-present modifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ItemId(NonZeroU16);

impl ItemId {
    pub fn new(value: u16) -> Result<Self, StationProtocolError> {
        NonZeroU16::new(value)
            .map(Self)
            .ok_or(StationProtocolError::ItemId)
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

/// Probability assigned to a server-selected result item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentChance {
    Always,
    Half,
    Quarter,
    TenPercent,
    TwoPercent,
}

impl PresentChance {
    const fn modifier_id(self) -> u16 {
        match self {
            Self::Always => 0x13,
            Self::Half => 0x14,
            Self::Quarter => 0x15,
            Self::TenPercent => 0x16,
            Self::TwoPercent => 0x17,
        }
    }
}

/// One or more characters selected by a character-specific modifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CharacterSelection(u16);

impl CharacterSelection {
    pub const GILGAMESH: Self = Self(0x0800);
    pub const WALKURE: Self = Self(0x1000);
    pub const YOUNG_KI: Self = Self(0x2000);
    pub const XEOVALGA: Self = Self(0x4000);
    pub const ALL: Self = Self(0x7800);

    const fn bits(self) -> u16 {
        self.0
    }
}

impl BitOr for CharacterSelection {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Defense fields that use the same percentage multiplier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefenseAttributes(u16);

impl DefenseAttributes {
    pub const PHYSICAL: Self = Self(0x0100);
    pub const MAGIC: Self = Self(0x0080);
    pub const BOTH: Self = Self(0x0180);

    const fn bits(self) -> u16 {
        self.0
    }
}

impl BitOr for DefenseAttributes {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Character fields that use one signed 16-bit point adjustment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CharacterPointAttributes(u16);

impl CharacterPointAttributes {
    pub const CRITICAL_RATE: Self = Self(0x0200);
    pub const EVASION_RATE: Self = Self(0x0040);
    pub const RESISTANCE_RATE: Self = Self(0x0020);
    pub const RETALIATION_DAMAGE: Self = Self(0x0010);

    const fn bits(self) -> u16 {
        self.0
    }
}

impl BitOr for CharacterPointAttributes {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Character fields that add one signed percentage-point adjustment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CharacterPercentAttributes(u16);

impl CharacterPercentAttributes {
    pub const MOVEMENT_SPEED: Self = Self(0x0008);
    pub const CASTING_SPEED: Self = Self(0x0004);
    pub const PHYSICAL_DAMAGE_RECEIVED: Self = Self(0x0002);
    pub const MAGIC_DAMAGE_RECEIVED: Self = Self(0x0001);

    const fn bits(self) -> u16 {
        self.0
    }
}

impl BitOr for CharacterPercentAttributes {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// One modifier ID/value pair in a matching activation response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuestModifier {
    None,
    ExperienceRewardMultiplier(u32),
    MoneyRewardMultiplier(u32),
    ItemDropRateMultiplier(u32),
    PresentItem {
        chance: PresentChance,
        item_id: ItemId,
    },
    RevealHiddenMonsters,
    EnemyHealthMultiplier(u32),
    EnemyAttackMultiplier(u32),
    PlayerMaxHpMultiplier(u32),
    PlayerMaxApMultiplier(u32),
    PlayerWeaponAttackMultiplier(u32),
    CharacterAttackSpeedMultiplier {
        characters: CharacterSelection,
        percent: NonZeroU32,
    },
    CharacterDefenseMultiplier {
        characters: CharacterSelection,
        attributes: DefenseAttributes,
        percent: u32,
    },
    CharacterPointBonus {
        characters: CharacterSelection,
        attributes: CharacterPointAttributes,
        points: i16,
    },
    CharacterPercentBonus {
        characters: CharacterSelection,
        attributes: CharacterPercentAttributes,
        percentage_points: i32,
    },
}

impl QuestModifier {
    pub(super) fn wire_pair(self) -> (u16, u32) {
        match self {
            Self::None => (0, 0),
            Self::ExperienceRewardMultiplier(percent) => (0x10, percent),
            Self::MoneyRewardMultiplier(percent) => (0x11, percent),
            Self::ItemDropRateMultiplier(percent) => (0x12, percent),
            Self::PresentItem { chance, item_id } => (chance.modifier_id(), item_id.get() as u32),
            Self::RevealHiddenMonsters => (0x1f, 0),
            Self::EnemyHealthMultiplier(percent) => (0x20, percent),
            Self::EnemyAttackMultiplier(percent) => (0x21, percent),
            Self::PlayerMaxHpMultiplier(percent) => (0x30, percent),
            Self::PlayerMaxApMultiplier(percent) => (0x31, percent),
            Self::PlayerWeaponAttackMultiplier(percent) => (0x32, percent),
            Self::CharacterAttackSpeedMultiplier {
                characters,
                percent,
            } => (0x8000 | characters.bits() | 0x0400, percent.get()),
            Self::CharacterDefenseMultiplier {
                characters,
                attributes,
                percent,
            } => (0x8000 | characters.bits() | attributes.bits(), percent),
            Self::CharacterPointBonus {
                characters,
                attributes,
                points,
            } => (
                0x8000 | characters.bits() | attributes.bits(),
                u32::from_ne_bytes(i32::from(points).to_ne_bytes()),
            ),
            Self::CharacterPercentBonus {
                characters,
                attributes,
                percentage_points,
            } => (
                0x8000 | characters.bits() | attributes.bits(),
                u32::from_ne_bytes(percentage_points.to_ne_bytes()),
            ),
        }
    }
}

/// One active quest and its four server-controlled event modifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuestEventConfiguration<QuestId> {
    quest_id: QuestId,
    modifiers: [QuestModifier; 4],
}

impl<QuestId> QuestEventConfiguration<QuestId> {
    pub const fn new(quest_id: QuestId, modifiers: [QuestModifier; 4]) -> Self {
        Self {
            quest_id,
            modifiers,
        }
    }

    pub const fn without_modifiers(quest_id: QuestId) -> Self {
        Self::new(quest_id, [QuestModifier::None; 4])
    }
}

/// Active quests and their event modifiers sent after matching-service activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchingActivationConfiguration {
    party_quests: [QuestEventConfiguration<PartyQuestId>; 2],
    special_quest: QuestEventConfiguration<SpecialQuestId>,
}

impl MatchingActivationConfiguration {
    pub const fn new(
        party_quests: [QuestEventConfiguration<PartyQuestId>; 2],
        special_quest: QuestEventConfiguration<SpecialQuestId>,
    ) -> Self {
        Self {
            party_quests,
            special_quest,
        }
    }
}

/// Wire-only layout parsed by `ParseTowerType06Configuration` in the Station.
#[derive(BinRead, BinWrite, Clone, Debug, Eq, PartialEq)]
#[br(big)]
#[bw(big)]
pub(super) struct MatchingActivationConfigurationWire {
    pub(super) party_quest_ids: [u16; 2],
    pub(super) special_quest_id: u16,
    pub(super) party_entry_attributes: [[u16; 4]; 2],
    pub(super) special_entry_attributes: [u16; 4],
    pub(super) reserved: u16,
    pub(super) party_entry_values: [[u32; 4]; 2],
    pub(super) special_entry_values: [u32; 4],
}

impl From<&MatchingActivationConfiguration> for MatchingActivationConfigurationWire {
    fn from(configuration: &MatchingActivationConfiguration) -> Self {
        let party_pairs = configuration
            .party_quests
            .map(|quest| quest.modifiers.map(QuestModifier::wire_pair));
        let special_pairs = configuration
            .special_quest
            .modifiers
            .map(QuestModifier::wire_pair);
        Self {
            party_quest_ids: configuration.party_quests.map(|quest| quest.quest_id.get()),
            special_quest_id: configuration.special_quest.quest_id.get(),
            party_entry_attributes: party_pairs.map(|pairs| pairs.map(|pair| pair.0)),
            special_entry_attributes: special_pairs.map(|pair| pair.0),
            reserved: 0,
            party_entry_values: party_pairs.map(|pairs| pairs.map(|pair| pair.1)),
            special_entry_values: special_pairs.map(|pair| pair.1),
        }
    }
}
