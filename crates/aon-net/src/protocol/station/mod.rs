mod event;
mod gameplay;
mod matching;
mod types;

pub(crate) use event::{ItemId, PresentChance, QuestModifier};
pub use event::{MatchingActivationConfiguration, QuestEventConfiguration};
pub use gameplay::{GameplayRequest, GameplayResponse, deserialize_gameplay_request};
pub use matching::{MatchingRequest, MatchingResponse, deserialize_matching_request};
pub use types::{
    EndpointAssignment, EndpointHost, GameplayBlob, GameplayEnvelopeFlags, LobbyLookup,
    LobbyRegistration, OwnerKey, ParticipantRecord, PartyRoster, PartySlot, PlayerRecord,
    RosterReadiness, StationProtocolError,
};
pub(crate) use types::{MAX_ENVELOPE_RECORDS, PlayerIdentity};

#[cfg(test)]
use event::{
    CharacterPercentAttributes, CharacterPointAttributes, CharacterSelection, DefenseAttributes,
};

#[cfg(test)]
pub use types::FixedText;

#[cfg(test)]
mod tests;
