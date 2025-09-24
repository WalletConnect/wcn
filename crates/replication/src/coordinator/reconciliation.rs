use {
    super::{Response, MAJORITY_QUORUM_THRESHOLD, RF},
    smallvec::SmallVec,
    std::collections::HashMap,
    wcn_storage_api::{operation, MapPage, Operation},
};

pub(super) fn reconcile(
    operation: &Operation<'_>,
    responses: &[Option<Response>],
    replicas_per_response: &[u8],
) -> Option<operation::Output> {
    use operation::{Borrowed, Owned};

    let reconcile_page = || reconcile_map_page(responses, replicas_per_response).map(Into::into);

    let reconcile_card =
        || reconcile_map_cardinality(responses, replicas_per_response).map(Into::into);

    match operation {
        Operation::Owned(owned) => match owned {
            Owned::HScan(_) => reconcile_page(),
            Owned::HCard(_) => reconcile_card(),
            Owned::Get(_)
            | Owned::Set(_)
            | Owned::Del(_)
            | Owned::GetExp(_)
            | Owned::SetExp(_)
            | Owned::HGet(_)
            | Owned::HSet(_)
            | Owned::HDel(_)
            | Owned::HGetExp(_)
            | Owned::HSetExp(_) => None,
        },
        Operation::Borrowed(borrowed) => match borrowed {
            Borrowed::HScan(_) => reconcile_page(),
            Borrowed::HCard(_) => reconcile_card(),
            Borrowed::Get(_)
            | Borrowed::Set(_)
            | Borrowed::Del(_)
            | Borrowed::GetExp(_)
            | Borrowed::SetExp(_)
            | Borrowed::HGet(_)
            | Borrowed::HSet(_)
            | Borrowed::HDel(_)
            | Borrowed::HGetExp(_)
            | Borrowed::HSetExp(_) => None,
        },
    }
}

// TODO: We need to figure out a unified way to handle read repair and
// reconciliation for maps.
//
/// Innefficent, but we currently have < 1 map reconciliation per second within
/// the entire network.
pub(super) fn reconcile_map_page(
    responses: &[Option<Response>],
    replicas_per_response: &[u8],
) -> Option<MapPage> {
    let iter = || {
        responses
            .iter()
            .enumerate()
            .filter_map(|(idx, opt)| match opt.as_ref()?.as_ref().ok()? {
                operation::Output::MapPage(page) => Some((page, replicas_per_response[idx])),
                _ => None,
            })
    };

    if iter().map(|(_, weight)| weight as usize).sum::<usize>() < MAJORITY_QUORUM_THRESHOLD {
        return None;
    }

    let has_next = iter()
        .map(|(page, weight)| if page.has_next { weight as usize } else { 0 })
        .sum::<usize>()
        >= MAJORITY_QUORUM_THRESHOLD;

    let size = iter()
        .map(|(page, _)| page.entries.len())
        .max()
        .unwrap_or_default();

    let counters = iter().fold(
        HashMap::with_capacity(size),
        |mut counters, (page, weight)| {
            page.entries.iter().for_each(|entry| {
                *counters.entry(entry).or_insert(0) += usize::from(weight);
            });

            counters
        },
    );

    let mut entries: Vec<_> = counters
        .into_iter()
        .filter(|(_, count)| *count >= MAJORITY_QUORUM_THRESHOLD)
        .map(|(entry, _)| entry.clone())
        .collect();

    entries.sort_unstable_by(|a, b| a.field.cmp(&b.field));

    Some(MapPage { entries, has_next })
}

pub(super) fn reconcile_map_cardinality(
    responses: &[Option<Response>],
    replicas_per_response: &[u8],
) -> Option<u64> {
    let mut cards: SmallVec<[(u64, u8); RF]> = responses
        .iter()
        .enumerate()
        .filter_map(|(idx, resp)| match resp.as_ref()?.as_ref().ok()? {
            operation::Output::Cardinality(card) => Some((*card, replicas_per_response[idx])),
            _ => None,
        })
        .collect();

    cards.sort();

    let mut total_weight = 0;
    for (card, weight) in cards {
        total_weight += weight;

        if total_weight as usize >= MAJORITY_QUORUM_THRESHOLD {
            return Some(card);
        }
    }

    None
}

#[cfg(test)]
mod test {
    use {
        super::*,
        wcn_storage_api::{MapEntry, Record, RecordExpiration, RecordVersion},
    };

    #[test]
    fn reconcile_map_page() {
        let expiration = RecordExpiration::from(std::time::Duration::from_secs(60));
        let version = RecordVersion::now();

        let page = |values: &[u8], has_next| MapPage {
            entries: values
                .iter()
                .map(|&byte| MapEntry {
                    field: vec![byte],
                    record: Record {
                        value: vec![byte],
                        expiration,
                        version,
                    },
                })
                .collect(),
            has_next,
        };

        let response = |values, has_next| Ok(operation::Output::MapPage(page(values, has_next)));

        let assert_page = |responses: &_, replicas_per_response: [_; 5], values, has_next| {
            assert_eq!(
                super::reconcile_map_page(responses, &replicas_per_response),
                Some(page(values, has_next))
            )
        };

        let responses = [
            Some(response(&[0, 1, 2], true)),
            Some(response(&[0, 1], true)),
            Some(response(&[0, 1, 3], true)),
            None,
            None,
        ];
        assert_page(&responses, [2, 1, 2, 0, 0], &[0, 1], true);

        let responses = [
            Some(response(&[0, 1, 2], false)),
            Some(response(&[0, 1, 2], true)),
            Some(response(&[0, 1, 3], false)),
            None,
            None,
        ];
        assert_page(&responses, [2, 1, 2, 0, 0], &[0, 1, 2], false);

        let responses = [
            Some(response(&[2, 4], false)),
            Some(response(&[0, 1], true)),
            Some(response(&[0, 1, 3], true)),
            Some(response(&[0, 1, 4], false)),
            None,
        ];
        assert_page(&responses, [2, 1, 1, 1, 0], &[0, 1, 4], false);

        let responses = [
            Some(response(&[0, 1, 2], true)),
            Some(response(&[1, 2, 3], true)),
            Some(response(&[2, 3, 4], true)),
            Some(response(&[4, 5, 6], true)),
            Some(response(&[7, 8, 9], false)),
        ];
        assert_page(&responses, [1, 1, 1, 1, 1], &[2], true);
    }

    #[test]
    fn reconcile_map_cardinality() {
        let response = |value| Ok(operation::Output::Cardinality(value));

        let assert_cardinality = |responses: &_, replicas_per_response: [_; 5], value| {
            assert_eq!(
                super::reconcile_map_cardinality(responses, &replicas_per_response),
                Some(value)
            );
        };

        let responses = [
            Some(response(0)),
            Some(response(1)),
            Some(response(2)),
            None,
            None,
        ];
        assert_cardinality(&responses, [2, 1, 2, 0, 0], 1);

        let responses = [
            Some(response(0)),
            Some(response(10)),
            Some(response(42)),
            None,
            None,
        ];
        assert_cardinality(&responses, [2, 2, 1, 0, 0], 10);

        let responses = [
            Some(response(0)),
            Some(response(1)),
            Some(response(2)),
            Some(response(3)),
            None,
        ];
        assert_cardinality(&responses, [1, 1, 1, 2, 0], 2);

        let responses = [
            Some(response(0)),
            Some(response(1)),
            Some(response(2)),
            Some(response(3)),
            Some(response(4)),
        ];
        assert_cardinality(&responses, [1, 1, 1, 1, 1], 2)
    }
}
