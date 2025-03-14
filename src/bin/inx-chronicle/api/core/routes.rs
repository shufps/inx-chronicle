// Copyright 2022 IOTA Stiftung
// SPDX-License-Identifier: Apache-2.0

use std::str::FromStr;

use axum::{
    extract::{Extension, Path},
    handler::Handler,
    http::header::HeaderMap,
    routing::get,
};
use chronicle::{
    db::{
        mongodb::collections::{
            BlockCollection, ConfigurationUpdateCollection, MilestoneCollection, OutputCollection,
            OutputMetadataResult, OutputWithMetadataResult, ProtocolUpdateCollection, TreasuryCollection,
            UtxoChangesResult,
        },
        MongoDb,
    },
    model::{
        metadata::BlockMetadata,
        payload::{MilestoneId, TransactionId},
        tangle::MilestoneIndex,
        utxo::OutputId,
        BlockId, TryFromWithContext,
    },
};
use futures::TryStreamExt;
use iota_types::{
    api::core::{
        dto::ReceiptDto,
        response::{
            self as iota, BaseTokenResponse, BlockMetadataResponse, ConfirmedMilestoneResponse,
            LatestMilestoneResponse, OutputWithMetadataResponse, ReceiptsResponse, StatusResponse, TreasuryResponse,
            UtxoChangesResponse,
        },
    },
    block::{
        output::dto::{OutputMetadataDto, RentStructureDto},
        payload::{dto::MilestonePayloadDto, milestone::option::dto::MilestoneOptionDto},
        protocol::dto::ProtocolParametersDto,
        BlockDto,
    },
};
use packable::PackableExt;

use super::responses::{InfoResponse, IotaRawResponse, IotaResponse};
use crate::api::{
    error::{ApiError, CorruptStateError, MissingError, RequestError},
    router::Router,
    routes::{is_healthy, not_implemented, BYTE_CONTENT_HEADER},
    ApiResult,
};

pub fn routes() -> Router {
    Router::new()
        .route("/info", get(info))
        .route("/tips", not_implemented.into_service())
        .nest(
            "/blocks",
            Router::new()
                .route("/", not_implemented.into_service())
                .route("/:block_id", get(block))
                .route("/:block_id/metadata", get(block_metadata)),
        )
        .nest(
            "/outputs",
            Router::new()
                .route("/:output_id", get(output))
                .route("/:output_id/metadata", get(output_metadata)),
        )
        .nest(
            "/receipts",
            Router::new()
                .route("/", get(receipts))
                .route("/:migrated_at", get(receipts_migrated_at)),
        )
        .route("/treasury", get(treasury))
        .nest(
            "/transactions",
            Router::new()
                .route("/:transaction_id/included-block", get(included_block))
                .route("/:transaction_id/included-block/metadata", get(included_block_metadata)),
        )
        .nest(
            "/milestones",
            Router::new()
                .route("/:milestone_id", get(milestone))
                .route("/:milestone_id/utxo-changes", get(utxo_changes))
                .route("/by-index/:index", get(milestone_by_index))
                .route("/by-index/:index/utxo-changes", get(utxo_changes_by_index)),
        )
        .nest(
            "/peers",
            Router::new()
                .route("/", not_implemented.into_service())
                .route("/:peer_id", not_implemented.into_service()),
        )
        .route("/control/database/prune", not_implemented.into_service())
        .route("/control/snapshot/create", not_implemented.into_service())
}

pub async fn info(database: Extension<MongoDb>) -> ApiResult<InfoResponse> {
    let protocol = database
        .collection::<ProtocolUpdateCollection>()
        .get_latest_protocol_parameters()
        .await?
        .ok_or(CorruptStateError::ProtocolParams)?
        .parameters;

    let is_healthy = is_healthy(&database).await.unwrap_or_else(|ApiError { error, .. }| {
        tracing::error!("An error occured during health check: {error}");
        false
    });

    let newest_milestone = database
        .collection::<MilestoneCollection>()
        .get_newest_milestone()
        .await?
        .ok_or(CorruptStateError::Milestone)?;
    let oldest_milestone = database
        .collection::<MilestoneCollection>()
        .get_oldest_milestone()
        .await?
        .ok_or(CorruptStateError::Milestone)?;

    let latest_milestone = LatestMilestoneResponse {
        index: newest_milestone.milestone_index.0,
        timestamp: Some(newest_milestone.milestone_timestamp.0),
        milestone_id: Some(
            iota_types::block::payload::milestone::MilestoneId::from(
                database
                    .collection::<MilestoneCollection>()
                    .get_milestone_id(newest_milestone.milestone_index)
                    .await?
                    .ok_or(CorruptStateError::Milestone)?,
            )
            .to_string(),
        ),
    };

    // Unfortunately, there is a distinction between `LatestMilestoneResponse` and `ConfirmedMilestoneResponse` in Bee.
    let confirmed_milestone = ConfirmedMilestoneResponse {
        index: latest_milestone.index,
        timestamp: latest_milestone.timestamp,
        milestone_id: latest_milestone.milestone_id.clone(),
    };

    let base_token = database
        .collection::<ConfigurationUpdateCollection>()
        .get_latest_node_configuration()
        .await?
        .ok_or(CorruptStateError::NodeConfig)?
        .config
        .base_token;

    Ok(InfoResponse {
        name: chronicle::CHRONICLE_APP_NAME.into(),
        version: std::env!("CARGO_PKG_VERSION").to_string(),
        status: StatusResponse {
            is_healthy,
            latest_milestone,
            confirmed_milestone,
            pruning_index: oldest_milestone.milestone_index.0 - 1,
        },
        protocol: ProtocolParametersDto {
            protocol_version: protocol.version,
            network_name: protocol.network_name,
            below_max_depth: protocol.below_max_depth,
            bech32_hrp: protocol.bech32_hrp,
            min_pow_score: protocol.min_pow_score,
            rent_structure: RentStructureDto {
                v_byte_cost: protocol.rent_structure.v_byte_cost,
                v_byte_factor_data: protocol.rent_structure.v_byte_factor_data,
                v_byte_factor_key: protocol.rent_structure.v_byte_factor_key,
            },
            token_supply: protocol.token_supply.to_string(),
        },
        base_token: BaseTokenResponse {
            name: base_token.name,
            ticker_symbol: base_token.ticker_symbol,
            decimals: base_token.decimals as u8,
            unit: base_token.unit,
            subunit: Some(base_token.subunit),
            use_metric_prefix: base_token.use_metric_prefix,
        },
    })
}

async fn block(
    database: Extension<MongoDb>,
    Path(block_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<IotaRawResponse<BlockDto>> {
    let block_id = BlockId::from_str(&block_id).map_err(RequestError::from)?;

    if matches!(headers.get(axum::http::header::ACCEPT), Some(header) if header == BYTE_CONTENT_HEADER) {
        return Ok(IotaRawResponse::Raw(
            database
                .collection::<BlockCollection>()
                .get_block_raw(&block_id)
                .await?
                .ok_or(MissingError::NoResults)?,
        ));
    }

    let block = database
        .collection::<BlockCollection>()
        .get_block(&block_id)
        .await?
        .ok_or(MissingError::NoResults)?;

    Ok(IotaRawResponse::Json(block.into()))
}

fn create_block_metadata_response(block_id: BlockId, metadata: BlockMetadata) -> iota::BlockMetadataResponse {
    iota::BlockMetadataResponse {
        block_id: block_id.to_hex(),
        parents: metadata.parents.iter().map(BlockId::to_hex).collect(),
        is_solid: metadata.is_solid,
        referenced_by_milestone_index: Some(*metadata.referenced_by_milestone_index),
        milestone_index: Some(*metadata.milestone_index),
        ledger_inclusion_state: Some(metadata.inclusion_state.into()),
        conflict_reason: Some(metadata.conflict_reason as u8),
        should_promote: Some(metadata.should_promote),
        should_reattach: Some(metadata.should_reattach),
        white_flag_index: Some(metadata.white_flag_index),
    }
}

async fn block_metadata(
    database: Extension<MongoDb>,
    Path(block_id_str): Path<String>,
) -> ApiResult<IotaResponse<BlockMetadataResponse>> {
    let block_id = BlockId::from_str(&block_id_str).map_err(RequestError::from)?;
    let metadata = database
        .collection::<BlockCollection>()
        .get_block_metadata(&block_id)
        .await?
        .ok_or(MissingError::NoResults)?;

    Ok(create_block_metadata_response(block_id, metadata).into())
}

fn create_output_metadata_response(metadata: OutputMetadataResult, ledger_index: MilestoneIndex) -> OutputMetadataDto {
    OutputMetadataDto {
        block_id: metadata.block_id.to_hex(),
        transaction_id: metadata.output_id.transaction_id.to_hex(),
        output_index: metadata.output_id.index,
        is_spent: metadata.spent_metadata.is_some(),
        milestone_index_spent: metadata
            .spent_metadata
            .as_ref()
            .map(|spent_md| *spent_md.spent.milestone_index),
        milestone_timestamp_spent: metadata
            .spent_metadata
            .as_ref()
            .map(|spent_md| *spent_md.spent.milestone_timestamp),
        transaction_id_spent: metadata
            .spent_metadata
            .as_ref()
            .map(|spent_md| spent_md.transaction_id.to_hex()),
        milestone_index_booked: *metadata.booked.milestone_index,
        milestone_timestamp_booked: *metadata.booked.milestone_timestamp,
        ledger_index: ledger_index.0,
    }
}

async fn output(
    database: Extension<MongoDb>,
    Path(output_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<IotaRawResponse<OutputWithMetadataResponse>> {
    let ledger_index = database
        .collection::<MilestoneCollection>()
        .get_ledger_index()
        .await?
        .ok_or(MissingError::NoResults)?;
    let output_id = OutputId::from_str(&output_id).map_err(RequestError::from)?;

    let OutputWithMetadataResult { output, metadata } = database
        .collection::<OutputCollection>()
        .get_output_with_metadata(&output_id, ledger_index)
        .await?
        .ok_or(MissingError::NoResults)?;

    if matches!(headers.get(axum::http::header::ACCEPT), Some(header) if header == BYTE_CONTENT_HEADER) {
        let ctx = database
            .collection::<ProtocolUpdateCollection>()
            .get_protocol_parameters_for_ledger_index(metadata.booked.milestone_index)
            .await?
            .ok_or(MissingError::NoResults)?
            .parameters;

        return Ok(IotaRawResponse::Raw(output.raw(ctx)?));
    }

    let metadata = create_output_metadata_response(metadata, ledger_index);

    Ok(IotaRawResponse::Json(OutputWithMetadataResponse {
        metadata,
        output: output.into(),
    }))
}

async fn output_metadata(
    database: Extension<MongoDb>,
    Path(output_id): Path<String>,
) -> ApiResult<IotaResponse<OutputMetadataDto>> {
    let ledger_index = database
        .collection::<MilestoneCollection>()
        .get_ledger_index()
        .await?
        .ok_or(MissingError::NoResults)?;
    let output_id = OutputId::from_str(&output_id).map_err(RequestError::from)?;
    let metadata = database
        .collection::<OutputCollection>()
        .get_output_metadata(&output_id, ledger_index)
        .await?
        .ok_or(MissingError::NoResults)?;

    Ok(create_output_metadata_response(metadata, ledger_index).into())
}

async fn included_block(
    database: Extension<MongoDb>,
    Path(transaction_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<IotaRawResponse<BlockDto>> {
    let transaction_id = TransactionId::from_str(&transaction_id).map_err(RequestError::from)?;

    if matches!(headers.get(axum::http::header::ACCEPT), Some(header) if header == BYTE_CONTENT_HEADER) {
        return Ok(IotaRawResponse::Raw(
            database
                .collection::<BlockCollection>()
                .get_block_raw_for_transaction(&transaction_id)
                .await?
                .ok_or(MissingError::NoResults)?,
        ));
    }

    let block = database
        .collection::<BlockCollection>()
        .get_block_for_transaction(&transaction_id)
        .await?
        .ok_or(MissingError::NoResults)?
        .block;

    Ok(IotaRawResponse::Json(block.into()))
}

async fn included_block_metadata(
    database: Extension<MongoDb>,
    Path(transaction_id): Path<String>,
) -> ApiResult<IotaResponse<BlockMetadataResponse>> {
    let transaction_id = TransactionId::from_str(&transaction_id).map_err(RequestError::from)?;

    let res = database
        .collection::<BlockCollection>()
        .get_block_metadata_for_transaction(&transaction_id)
        .await?
        .ok_or(MissingError::NoResults)?;
    let block_id = res.block_id;
    let metadata = res.metadata;

    Ok(create_block_metadata_response(block_id, metadata).into())
}

async fn receipts(database: Extension<MongoDb>) -> ApiResult<IotaResponse<ReceiptsResponse>> {
    let mut receipts_at = database.collection::<MilestoneCollection>().get_all_receipts().await?;
    let mut receipts = Vec::new();
    while let Some((receipt, at)) = receipts_at.try_next().await? {
        if let MilestoneOptionDto::Receipt(receipt) = receipt.into() {
            receipts.push(ReceiptDto {
                receipt,
                milestone_index: *at,
            });
        } else {
            unreachable!("the query only returns receipt milestone options");
        }
    }
    Ok(iota::ReceiptsResponse { receipts }.into())
}

async fn receipts_migrated_at(
    database: Extension<MongoDb>,
    Path(index): Path<u32>,
) -> ApiResult<IotaResponse<ReceiptsResponse>> {
    let mut receipts_at = database
        .collection::<MilestoneCollection>()
        .get_receipts_migrated_at(index.into())
        .await?;
    let mut receipts = Vec::new();
    while let Some((receipt, at)) = receipts_at.try_next().await? {
        if let MilestoneOptionDto::Receipt(receipt) = receipt.into() {
            receipts.push(ReceiptDto {
                receipt,
                milestone_index: *at,
            });
        } else {
            unreachable!("the query only returns receipt milestone options");
        }
    }
    Ok(iota::ReceiptsResponse { receipts }.into())
}

async fn treasury(database: Extension<MongoDb>) -> ApiResult<IotaResponse<TreasuryResponse>> {
    Ok(database
        .collection::<TreasuryCollection>()
        .get_latest_treasury()
        .await?
        .ok_or(MissingError::NoResults)
        .map(|treasury| {
            iota::TreasuryResponse {
                milestone_id: treasury.milestone_id.to_hex(),
                amount: treasury.amount.to_string(),
            }
            .into()
        })?)
}

async fn milestone(
    database: Extension<MongoDb>,
    Path(milestone_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<IotaRawResponse<MilestonePayloadDto>> {
    let milestone_id = MilestoneId::from_str(&milestone_id).map_err(RequestError::from)?;
    let milestone_payload = database
        .collection::<MilestoneCollection>()
        .get_milestone_payload_by_id(&milestone_id)
        .await?
        .ok_or(MissingError::NoResults)?;

    if matches!(headers.get(axum::http::header::ACCEPT), Some(header) if header == BYTE_CONTENT_HEADER) {
        let protocol_params = database
            .collection::<ProtocolUpdateCollection>()
            .get_protocol_parameters_for_ledger_index(milestone_payload.essence.index)
            .await?
            .ok_or(MissingError::NoResults)?
            .parameters
            .try_into()?;

        let milestone_payload =
            iota_types::block::payload::MilestonePayload::try_from_with_context(&protocol_params, milestone_payload)?;

        return Ok(IotaRawResponse::Raw(milestone_payload.pack_to_vec()));
    }

    Ok(IotaRawResponse::Json(milestone_payload.into()))
}

async fn milestone_by_index(
    database: Extension<MongoDb>,
    Path(index): Path<MilestoneIndex>,
    headers: HeaderMap,
) -> ApiResult<IotaRawResponse<MilestonePayloadDto>> {
    let milestone_payload = database
        .collection::<MilestoneCollection>()
        .get_milestone_payload(index)
        .await?
        .ok_or(MissingError::NoResults)?;

    if matches!(headers.get(axum::http::header::ACCEPT), Some(header) if header == BYTE_CONTENT_HEADER) {
        let protocol_params = database
            .collection::<ProtocolUpdateCollection>()
            .get_protocol_parameters_for_ledger_index(milestone_payload.essence.index)
            .await?
            .ok_or(MissingError::NoResults)?
            .parameters
            .try_into()?;

        let milestone_payload =
            iota_types::block::payload::MilestonePayload::try_from_with_context(&protocol_params, milestone_payload)?;

        return Ok(IotaRawResponse::Raw(milestone_payload.pack_to_vec()));
    }

    Ok(IotaRawResponse::Json(milestone_payload.into()))
}

async fn utxo_changes(
    database: Extension<MongoDb>,
    Path(milestone_id): Path<String>,
) -> ApiResult<IotaResponse<UtxoChangesResponse>> {
    let milestone_id = MilestoneId::from_str(&milestone_id).map_err(RequestError::from)?;
    let milestone_index = database
        .collection::<MilestoneCollection>()
        .get_milestone_payload_by_id(&milestone_id)
        .await?
        .ok_or(MissingError::NoResults)?
        .essence
        .index;
    collect_utxo_changes(&database, milestone_index).await.map(Into::into)
}

async fn utxo_changes_by_index(
    database: Extension<MongoDb>,
    Path(milestone_index): Path<MilestoneIndex>,
) -> ApiResult<IotaResponse<UtxoChangesResponse>> {
    collect_utxo_changes(&database, milestone_index).await.map(Into::into)
}

async fn collect_utxo_changes(database: &MongoDb, milestone_index: MilestoneIndex) -> ApiResult<UtxoChangesResponse> {
    let ledger_index = database
        .collection::<MilestoneCollection>()
        .get_ledger_index()
        .await?
        .ok_or(MissingError::NoResults)?;
    let UtxoChangesResult {
        created_outputs,
        consumed_outputs,
    } = database
        .collection::<OutputCollection>()
        .get_utxo_changes(milestone_index, ledger_index)
        .await?
        .ok_or(MissingError::NoResults)?;

    let created_outputs = created_outputs.iter().map(|output_id| output_id.to_hex()).collect();
    let consumed_outputs = consumed_outputs.iter().map(|output_id| output_id.to_hex()).collect();

    Ok(iota::UtxoChangesResponse {
        index: *milestone_index,
        created_outputs,
        consumed_outputs,
    })
}
