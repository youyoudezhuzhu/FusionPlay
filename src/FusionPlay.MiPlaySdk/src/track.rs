//! Track identity for hosts merging partial MiPlay media_info events.

pub struct TrackCredit<'a> {
    pub id: Option<&'a str>,
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
}

impl TrackCredit<'_> {
    // The SDK's metadata: IDs hash optional fields and can vary within one song.
    pub fn sender_id(&self) -> Option<&str> {
        self.id
            .filter(|id| !id.is_empty() && !id.starts_with("metadata:"))
    }

    pub fn changed_from(&self, previous: &Self) -> bool {
        let differs =
            |old: Option<&str>, new: Option<&str>| old.zip(new).is_some_and(|(a, b)| a != b);
        match (previous.sender_id(), self.sender_id()) {
            (Some(old), Some(new)) => old != new,
            _ => differs(previous.title, self.title) || differs(previous.artist, self.artist),
        }
    }
}
