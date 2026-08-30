use fastrand::Rng;

use crate::protocol::tower::{PartyQuestId, SpecialQuestId, TowerProtocolError};

const FIRST_PARTY_QUEST_ID: u16 = 10;
const LAST_PARTY_QUEST_ID: u16 = 22;
const PARTY_QUEST_COUNT: usize = (LAST_PARTY_QUEST_ID - FIRST_PARTY_QUEST_ID + 1) as usize;
const FIRST_SPECIAL_QUEST_ID: u16 = 25;
const FIRST_SPECIAL_QUEST_END: u16 = 74;
const SECOND_SPECIAL_QUEST_START: u16 = 76;
const LAST_SPECIAL_QUEST_ID: u16 = 89;
const SPECIAL_QUEST_COUNT: usize = (FIRST_SPECIAL_QUEST_END - FIRST_SPECIAL_QUEST_ID + 1) as usize
    + (LAST_SPECIAL_QUEST_ID - SECOND_SPECIAL_QUEST_START + 1) as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RandomQuestRotation {
    pub(crate) active_party: [PartyQuestId; 2],
    pub(crate) active_special: SpecialQuestId,
    pub(crate) normal_quest: PartyQuestId,
    pub(crate) hard_quest: PartyQuestId,
}

impl RandomQuestRotation {
    pub(crate) fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
    ) -> Result<Self, TowerProtocolError> {
        let mut random = Rng::with_seed(rotation_seed(year, month, day, hour, minute));
        let mut party_pool: [u16; PARTY_QUEST_COUNT] =
            std::array::from_fn(|index| FIRST_PARTY_QUEST_ID + index as u16);
        random.shuffle(&mut party_pool);
        let mut special_pool: [u16; SPECIAL_QUEST_COUNT] = std::array::from_fn(|index| {
            let quest_id = FIRST_SPECIAL_QUEST_ID + index as u16;
            if quest_id > FIRST_SPECIAL_QUEST_END {
                quest_id + (SECOND_SPECIAL_QUEST_START - FIRST_SPECIAL_QUEST_END - 1)
            } else {
                quest_id
            }
        });
        random.shuffle(&mut special_pool);

        Ok(Self {
            active_party: [
                PartyQuestId::new(party_pool[0])?,
                PartyQuestId::new(party_pool[1])?,
            ],
            active_special: SpecialQuestId::new(special_pool[0])?,
            normal_quest: PartyQuestId::new(party_pool[2])?,
            hard_quest: PartyQuestId::new(party_pool[3])?,
        })
    }
}

const fn rotation_seed(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> u64 {
    let hourly = (year as u64) << 24 | (month as u64) << 16 | (day as u64) << 8 | hour as u64;
    hourly ^ ((minute as u64) << 40)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn rotation_is_stable_for_one_hour_and_changes_for_the_next_hour()
    -> Result<(), TowerProtocolError> {
        let first = RandomQuestRotation::new(2026, 8, 21, 12, 0)?;
        let repeated = RandomQuestRotation::new(2026, 8, 21, 12, 0)?;
        let next = RandomQuestRotation::new(2026, 8, 21, 13, 0)?;

        assert_eq!(first, repeated);
        assert_ne!(first, next);
        Ok(())
    }

    #[test]
    fn rotation_uses_distinct_party_quests_and_the_correct_families()
    -> Result<(), TowerProtocolError> {
        for hour in 0..24 {
            let rotation = RandomQuestRotation::new(2026, 8, 21, hour, 0)?;
            let party_ids = [
                rotation.active_party[0].get(),
                rotation.active_party[1].get(),
                rotation.normal_quest.get(),
                rotation.hard_quest.get(),
            ];
            assert_eq!(party_ids.into_iter().collect::<HashSet<_>>().len(), 4);
            assert!(party_ids.into_iter().all(|id| (10..=22).contains(&id)));
            let special_id = rotation.active_special.get();
            assert!((25..=74).contains(&special_id) || (76..=89).contains(&special_id));
        }
        Ok(())
    }
}
