use anyhow::Context;
use nostr::event;
use nostr::Event;
use nostr::EventId;
use nostr::UncheckedUrl;

pub fn get_zapped_note_id(zap_request: &Event) -> anyhow::Result<EventId> {
    let tags = zap_request.tags();
    let tags = tags
        .iter()
        .filter_map(|tag| match tag.as_standardized() {
            Some(event::TagStandard::Event { event_id, .. }) => Some(*event_id),
            _ => None,
        })
        .collect::<Vec<_>>();

    let zapped_note = tags
        // first is ok here, because there should only be one event (if any)
        .first()
        .context("can only accept zaps on notes.")?;

    Ok(*zapped_note)
}

pub fn get_relays(zap_request: &Event) -> anyhow::Result<Vec<String>> {
    let tags = zap_request.tags();
    let relays = tags
        .iter()
        .filter_map(|tag| match tag.as_standardized() {
            Some(event::TagStandard::Relays(relays)) => Some(relays.clone()),
            Some(event::TagStandard::Relay(relay)) => Some(vec![relay.clone()]),
            _ => None,
        })
        .collect::<Vec<Vec<UncheckedUrl>>>();

    let relays: Vec<_> = relays.into_iter().flatten().collect();
    let relays: Vec<_> = relays.into_iter().map(|r| r.to_string()).collect();

    Ok(relays)
}
