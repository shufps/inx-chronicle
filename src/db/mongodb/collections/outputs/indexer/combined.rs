// Copyright 2024 IOTA Stiftung
// SPDX-License-Identifier: Apache-2.0

use mongodb::bson::{self, doc};
use primitive_types::U256;

use super::queries::{
    AddressQuery, AliasUnlockableByAddressQuery, AppendQuery, CreatedQuery, NativeTokensQuery, UnlockableByAddressQuery,
};
use crate::model::{tangle::MilestoneTimestamp, utxo::Address};

/// A query over all indexed output kinds at once, mirroring the combined `/outputs` route of the `inx-indexer`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct OutputsQuery {
    pub unlockable_by_address: Option<Address>,
    pub has_native_tokens: Option<bool>,
    pub min_native_token_count: Option<U256>,
    pub max_native_token_count: Option<U256>,
    pub created_before: Option<MilestoneTimestamp>,
    pub created_after: Option<MilestoneTimestamp>,
}

impl From<OutputsQuery> for bson::Document {
    fn from(query: OutputsQuery) -> Self {
        let address = query.unlockable_by_address;

        let mut basic = vec![doc! { "output.kind": "basic" }];
        basic.append_query(UnlockableByAddressQuery(address));

        let mut alias = vec![doc! { "output.kind": "alias" }];
        alias.append_query(AliasUnlockableByAddressQuery(address));

        let mut nft = vec![doc! { "output.kind": "nft" }];
        nft.append_query(UnlockableByAddressQuery(address));

        let mut kinds = vec![doc! { "$and": basic }, doc! { "$and": alias }, doc! { "$and": nft }];

        // Foundries can only be unlocked by their controlling alias, so they are only included if no address or an
        // alias address is given.
        if matches!(address, None | Some(Address::Alias(_))) {
            let mut foundry = vec![doc! { "output.kind": "foundry" }];
            foundry.append_query(AddressQuery(address));
            kinds.push(doc! { "$and": foundry });
        }

        let mut queries = vec![doc! { "$or": kinds }];
        queries.append_query(NativeTokensQuery {
            has_native_tokens: query.has_native_tokens,
            min_native_token_count: query.min_native_token_count,
            max_native_token_count: query.max_native_token_count,
        });
        queries.append_query(CreatedQuery {
            created_before: query.created_before,
            created_after: query.created_after,
        });
        doc! { "$and": queries }
    }
}

#[cfg(all(test, feature = "rand"))]
mod test {
    use mongodb::bson::{self, doc};
    use pretty_assertions::assert_eq;

    use super::OutputsQuery;
    use crate::model::utxo::Address;

    #[test]
    fn test_outputs_query_ed25519_address() {
        let address = Address::rand_ed25519();
        let query = OutputsQuery {
            unlockable_by_address: Some(address),
            has_native_tokens: Some(false),
            created_before: Some(10000.into()),
            ..Default::default()
        };
        let query_doc = doc! {
            "$and": [
                { "$or": [
                    { "$and": [
                        { "output.kind": "basic" },
                        { "$or": [
                            { "details.address": address },
                            { "output.expiration_unlock_condition.return_address": address },
                            { "output.storage_deposit_return_unlock_condition.return_address": address },
                        ] },
                    ] },
                    { "$and": [
                        { "output.kind": "alias" },
                        { "$or": [
                            { "details.address": address },
                            { "output.governor_address_unlock_condition.address": address },
                        ] },
                    ] },
                    { "$and": [
                        { "output.kind": "nft" },
                        { "$or": [
                            { "details.address": address },
                            { "output.expiration_unlock_condition.return_address": address },
                            { "output.storage_deposit_return_unlock_condition.return_address": address },
                        ] },
                    ] },
                ] },
                { "output.native_tokens": { "$eq": [] } },
                { "metadata.booked.milestone_timestamp": { "$lt": 10000 } },
            ]
        };
        assert_eq!(query_doc, bson::Document::from(query));
    }

    #[test]
    fn test_outputs_query_alias_address_includes_foundries() {
        let address = Address::rand_alias();
        let query = OutputsQuery {
            unlockable_by_address: Some(address),
            ..Default::default()
        };
        let query_doc = bson::Document::from(query);
        let kinds = query_doc.get_array("$and").unwrap()[0]
            .as_document()
            .unwrap()
            .get_array("$or")
            .unwrap();
        assert_eq!(kinds.len(), 4);
        assert_eq!(
            kinds[3],
            bson::Bson::from(doc! { "$and": [
                { "output.kind": "foundry" },
                { "details.address": address },
            ] })
        );
    }

    #[test]
    fn test_outputs_query_empty() {
        let query_doc = bson::Document::from(OutputsQuery::default());
        let query_doc_expected = doc! {
            "$and": [
                { "$or": [
                    { "$and": [{ "output.kind": "basic" }] },
                    { "$and": [{ "output.kind": "alias" }] },
                    { "$and": [{ "output.kind": "nft" }] },
                    { "$and": [{ "output.kind": "foundry" }] },
                ] },
            ]
        };
        assert_eq!(query_doc_expected, query_doc);
    }
}
