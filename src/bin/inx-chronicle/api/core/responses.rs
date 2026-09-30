// Copyright 2022 IOTA Stiftung
// SPDX-License-Identifier: Apache-2.0

use iota_sdk::types::{api::core::response as iota, block::protocol::ProtocolParameters};
use serde::{Deserialize, Serialize};

use crate::api::responses::impl_success_response;

/// Response of `GET /api/info`.
///
/// Mirrors the field set of [`iota::InfoResponse`] so that clients (e.g. the `iota-sdk`), which deserialize the node
/// info strictly, accept Chronicle as a drop-in replacement for a Hornet node.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoResponse {
    pub name: String,
    pub version: String,
    pub status: iota::StatusResponse,
    pub supported_protocol_versions: Vec<u8>,
    pub protocol: ProtocolParameters,
    pub pending_protocol_parameters: Vec<iota::PendingProtocolParameter>,
    pub base_token: iota::BaseTokenResponse,
    pub metrics: iota::MetricsResponse,
    pub features: Vec<String>,
}

impl_success_response!(InfoResponse);

/// A wrapper struct that allows us to implement [`IntoResponse`](axum::response::IntoResponse) for the foreign
/// responses from [`iota_types`](iota_sdk::types::api::core::response).
#[derive(Clone, Debug, Serialize, derive_more::From)]
pub struct IotaResponse<T: Serialize>(T);

impl<T: Serialize> axum::response::IntoResponse for IotaResponse<T> {
    fn into_response(self) -> axum::response::Response {
        axum::Json(self.0).into_response()
    }
}

/// A wrapper struct that allows us to implement [`IntoResponse`](axum::response::IntoResponse) for the foreign
/// raw responses from [`iota_types`](iota_sdk::types::api::core::response).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IotaRawResponse<T: Serialize> {
    Json(T),
    Raw(Vec<u8>),
}

impl<T: Serialize> axum::response::IntoResponse for IotaRawResponse<T> {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Json(res) => axum::Json(res).into_response(),
            Self::Raw(bytes) => bytes.into_response(),
        }
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Chronicle's info response must be accepted by the `iota-sdk` exactly like a real Hornet one.
    #[test]
    fn info_response_is_sdk_compatible() {
        // A real `/api/core/v2/info` response of a Hornet node on the Shimmer network.
        let hornet = serde_json::json!({
            "name": "HORNET",
            "version": "2.0.0",
            "status": {
                "isHealthy": false,
                "latestMilestone": {
                    "index": 24359373,
                    "timestamp": 1790754249,
                    "milestoneId": "0x327a7aa3db2a2de9ee02c34660e83fe85b02e117a7558292aab6048f617db5cd"
                },
                "confirmedMilestone": {
                    "index": 24359373,
                    "timestamp": 1790754249,
                    "milestoneId": "0x327a7aa3db2a2de9ee02c34660e83fe85b02e117a7558292aab6048f617db5cd"
                },
                "pruningIndex": 20696765
            },
            "supportedProtocolVersions": [2],
            "protocol": {
                "version": 2,
                "networkName": "shimmer",
                "bech32Hrp": "smr",
                "minPowScore": 1500,
                "belowMaxDepth": 15,
                "rentStructure": {
                    "vByteCost": 100,
                    "vByteFactorData": 1,
                    "vByteFactorKey": 10
                },
                "tokenSupply": "1813620509061365"
            },
            "pendingProtocolParameters": [],
            "baseToken": {
                "name": "Shimmer",
                "tickerSymbol": "SMR",
                "unit": "SMR",
                "subunit": "glow",
                "decimals": 6,
                "useMetricPrefix": false
            },
            "metrics": {
                "blocksPerSecond": 3.8,
                "referencedBlocksPerSecond": 2.8,
                "referencedRate": 73.68421052631578
            },
            "features": []
        });

        let chronicle: InfoResponse = serde_json::from_value(hornet.clone()).unwrap();
        let serialized = serde_json::to_value(&chronicle).unwrap();
        assert_eq!(serialized, hornet);

        let sdk: iota::InfoResponse = serde_json::from_value(serialized).unwrap();
        assert_eq!(serde_json::to_value(sdk).unwrap(), hornet);
    }
}
