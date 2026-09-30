// Copyright 2024 IOTA Stiftung
// SPDX-License-Identifier: Apache-2.0

mod common;

#[cfg(feature = "rand")]
mod test_rand {
    use std::collections::HashSet;

    use chronicle::{
        db::mongodb::collections::{
            AliasOutputsQuery, BasicOutputsQuery, NftOutputsQuery, OutputCollection, OutputsQuery, SortOrder,
        },
        model::{
            ledger::{LedgerOutput, LedgerSpent, RentStructureBytes},
            metadata::SpentMetadata,
            payload::TransactionId,
            tangle::MilestoneIndexTimestamp,
            utxo::{Address, FoundryOutput, Output, OutputId},
            BlockId,
        },
    };
    use iota_sdk::types::block::{
        address::Address as IotaAddress,
        output::{
            unlock_condition::{
                AddressUnlockCondition, ExpirationUnlockCondition, GovernorAddressUnlockCondition,
                StateControllerAddressUnlockCondition, StorageDepositReturnUnlockCondition,
            },
            AliasId, AliasOutputBuilder, BasicOutputBuilder, NftId, NftOutputBuilder,
        },
        protocol::protocol_parameters,
        rand::address::rand_ed25519_address,
    };
    use pretty_assertions::assert_eq;

    use super::common::{setup_collection, setup_database, teardown};

    const AMOUNT: u64 = 1_000_000;

    fn ledger_output(output: impl Into<Output>) -> LedgerOutput {
        LedgerOutput {
            output_id: OutputId::rand(),
            rent_structure: RentStructureBytes {
                num_key_bytes: 0,
                num_data_bytes: 100,
            },
            output: output.into(),
            block_id: BlockId::rand(),
            booked: MilestoneIndexTimestamp {
                milestone_index: 1.into(),
                milestone_timestamp: 12345.into(),
            },
        }
    }

    #[tokio::test]
    async fn test_unlockable_by_address() {
        let db = setup_database("test-unlockable-by-address").await.unwrap();
        let output_collection = setup_collection::<OutputCollection>(&db).await.unwrap();

        let token_supply = protocol_parameters().token_supply();
        let me = IotaAddress::from(rand_ed25519_address());
        let other = IotaAddress::from(rand_ed25519_address());

        let basic = |owner: IotaAddress| {
            BasicOutputBuilder::new_with_amount(AMOUNT).add_unlock_condition(AddressUnlockCondition::new(owner))
        };

        let basic_owned = ledger_output(&basic(me).finish_output(token_supply).unwrap());
        let basic_expiration_return = ledger_output(
            &basic(other)
                .add_unlock_condition(ExpirationUnlockCondition::new(me, 12345).unwrap())
                .finish_output(token_supply)
                .unwrap(),
        );
        let basic_storage_deposit_return = ledger_output(
            &basic(other)
                .add_unlock_condition(StorageDepositReturnUnlockCondition::new(me, AMOUNT / 2, token_supply).unwrap())
                .finish_output(token_supply)
                .unwrap(),
        );
        let basic_foreign = ledger_output(&basic(other).finish_output(token_supply).unwrap());
        let basic_spent = ledger_output(&basic(me).finish_output(token_supply).unwrap());

        let alias_governed = ledger_output(
            &AliasOutputBuilder::new_with_amount(AMOUNT, AliasId::null())
                .add_unlock_condition(StateControllerAddressUnlockCondition::new(other))
                .add_unlock_condition(GovernorAddressUnlockCondition::new(me))
                .finish_output(token_supply)
                .unwrap(),
        );
        let nft_owned = ledger_output(
            &NftOutputBuilder::new_with_amount(AMOUNT, NftId::null())
                .add_unlock_condition(AddressUnlockCondition::new(me))
                .finish_output(token_supply)
                .unwrap(),
        );
        let foundry_output = FoundryOutput::rand(&protocol_parameters());
        let foundry_alias = foundry_output.immutable_alias_address_unlock_condition.address;
        let foundry = ledger_output(Output::Foundry(foundry_output));

        output_collection
            .insert_unspent_outputs([
                &basic_owned,
                &basic_expiration_return,
                &basic_storage_deposit_return,
                &basic_foreign,
                &basic_spent,
                &alias_governed,
                &nft_owned,
                &foundry,
            ])
            .await
            .unwrap();
        output_collection
            .update_spent_outputs([&LedgerSpent {
                output: basic_spent.clone(),
                spent_metadata: SpentMetadata {
                    transaction_id: TransactionId::rand(),
                    spent: MilestoneIndexTimestamp {
                        milestone_index: 1.into(),
                        milestone_timestamp: 23456.into(),
                    },
                },
            }])
            .await
            .unwrap();

        let query = |q| {
            let output_collection = &output_collection;
            async move {
                output_collection
                    .get_indexed_outputs(q, 100, None, SortOrder::Newest, false, 1.into())
                    .await
                    .unwrap()
                    .outputs
                    .into_iter()
                    .map(|o| o.output_id)
                    .collect::<HashSet<_>>()
            }
        };
        let ids = |outputs: &[&LedgerOutput]| outputs.iter().map(|o| o.output_id).collect::<HashSet<_>>();
        let me = Address::from(me);

        assert_eq!(
            query(bson_doc(OutputsQuery {
                unlockable_by_address: Some(me),
                ..Default::default()
            }))
            .await,
            ids(&[
                &basic_owned,
                &basic_expiration_return,
                &basic_storage_deposit_return,
                &alias_governed,
                &nft_owned,
            ]),
        );
        assert_eq!(
            query(bson_doc(BasicOutputsQuery {
                unlockable_by_address: Some(me),
                ..Default::default()
            }))
            .await,
            ids(&[&basic_owned, &basic_expiration_return, &basic_storage_deposit_return]),
        );
        assert_eq!(
            query(bson_doc(AliasOutputsQuery {
                unlockable_by_address: Some(me),
                ..Default::default()
            }))
            .await,
            ids(&[&alias_governed]),
        );
        assert_eq!(
            query(bson_doc(NftOutputsQuery {
                unlockable_by_address: Some(me),
                ..Default::default()
            }))
            .await,
            ids(&[&nft_owned]),
        );
        // Foundries are only returned for their controlling alias address.
        assert_eq!(
            query(bson_doc(OutputsQuery {
                unlockable_by_address: Some(foundry_alias),
                ..Default::default()
            }))
            .await,
            ids(&[&foundry]),
        );

        teardown(db).await;
    }

    fn bson_doc(query: impl Into<mongodb::bson::Document>) -> mongodb::bson::Document {
        query.into()
    }
}
