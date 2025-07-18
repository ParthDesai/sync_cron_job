use crate::errors::{Result, SyncCronError};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct PinataClient {
    client: reqwest::Client,
    jwt_token: String,
    gateway_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PinataUploadResponseWrapper {
    pub data: PinataUploadResponse,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PinataUploadResponse {
    pub id: String,
    pub name: String,
    pub cid: String,
    pub size: u64,
    pub number_of_files: u32,
    pub mime_type: String,
    pub user_id: String,
    pub group_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PinataUploadResult {
    pub cid: String,
    pub gateway_url: String,
    pub ipfs_url: String,
}

impl PinataClient {
    /// Creates a new Pinata client
    ///
    /// # Arguments
    /// * `jwt_token` - Your Pinata JWT token
    /// * `gateway_url` - Optional custom gateway URL (defaults to Pinata's public gateway)
    pub fn new(jwt_token: String, gateway_url: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| SyncCronError::Network(format!("Failed to create HTTP client: {}", e)))?;

        Ok(PinataClient {
            client,
            jwt_token,
            gateway_url,
        })
    }

    /// Uploads raw bytes to Pinata IPFS
    ///
    /// # Arguments
    /// * `data` - Raw bytes to upload
    /// * `filename` - Name for the file (will be used in IPFS)
    /// * `mime_type` - Optional MIME type (will be auto-detected if not provided)
    ///
    /// # Returns
    /// A `PinataUploadResult` containing the CID and various URLs
    pub async fn upload_bytes(
        &self,
        data: Vec<u8>,
        filename: &str,
        mime_type: Option<&str>,
    ) -> Result<PinataUploadResult> {
        let detected_mime_type = mime_type.map(|mt| mt.to_string()).unwrap_or_else(|| {
            mime_guess::from_path(filename)
                .first_or_octet_stream()
                .to_string()
        });

        log::info!(
            "📤 Uploading file '{}' ({} bytes) to Pinata public IPFS network",
            filename,
            data.len()
        );

        // Create multipart form
        let part = Part::bytes(data)
            .file_name(filename.to_string())
            .mime_str(&detected_mime_type)
            .map_err(|e| SyncCronError::Network(format!("Failed to create multipart: {}", e)))?;

        let form = Form::new().part("file", part).text("network", "private");

        // Make the upload request
        let response = self
            .client
            .post("https://uploads.pinata.cloud/v3/files")
            .bearer_auth(&self.jwt_token)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                log::error!("❌ Upload request failed: {}", e);
                SyncCronError::Network(format!("Upload request failed: {}", e))
            })?;

        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| SyncCronError::Network(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            log::error!(
                "⛔ Pinata upload failed with status {}: {}",
                status,
                response_text
            );
            return Err(SyncCronError::Network(format!(
                "Upload failed with status {}: {}",
                status, response_text
            )));
        }

        // Parse the response
        let upload_response_wrapper: PinataUploadResponseWrapper =
            serde_json::from_str(&response_text).map_err(|e| {
                log::error!(
                    "🔍 Failed to parse Pinata response: {} with an error: {}",
                    response_text,
                    e
                );
                SyncCronError::Network(format!("Failed to parse response: {}", e))
            })?;

        let upload_response = upload_response_wrapper.data;

        log::info!(
            "✅ Successfully uploaded file to Pinata IPFS with CID: {}",
            upload_response.cid
        );

        // Create the result with various URL formats
        let result = PinataUploadResult {
            cid: upload_response.cid.clone(),
            gateway_url: format!("{}/ipfs/{}", self.gateway_url, upload_response.cid),
            ipfs_url: format!("ipfs://{}", upload_response.cid),
        };

        Ok(result)
    }

    /// Uploads a string as a text file to Pinata IPFS
    ///
    /// # Arguments
    /// * `content` - String content to upload
    /// * `filename` - Name for the file
    ///
    /// # Returns
    /// A `PinataUploadResult` containing the CID and various URLs
    pub async fn upload_text(&self, content: &str, filename: &str) -> Result<PinataUploadResult> {
        self.upload_bytes(content.as_bytes().to_vec(), filename, Some("text/plain"))
            .await
    }

    /// Uploads JSON data to Pinata IPFS
    ///
    /// # Arguments
    /// * `data` - Any serializable data
    /// * `filename` - Name for the JSON file
    ///
    /// # Returns
    /// A `PinataUploadResult` containing the CID and various URLs
    pub async fn upload_json<T: Serialize>(
        &self,
        data: &T,
        filename: &str,
    ) -> Result<PinataUploadResult> {
        let json_string = serde_json::to_string_pretty(data)
            .map_err(|e| SyncCronError::Network(format!("Failed to serialize JSON: {}", e)))?;

        self.upload_bytes(
            json_string.as_bytes().to_vec(),
            filename,
            Some("application/json"),
        )
        .await
    }

    /// Test authentication with Pinata API
    pub async fn test_authentication(&self) -> Result<()> {
        let response = self
            .client
            .get("https://uploads.pinata.cloud/v3/files")
            .bearer_auth(&self.jwt_token)
            .query(&[("limit", "1")])
            .send()
            .await
            .map_err(|e| SyncCronError::Network(format!("Auth test request failed: {}", e)))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(SyncCronError::Network(format!(
                "Authentication failed with status: {}",
                response.status()
            )))
        }
    }

    /// Get the gateway URL being used
    pub fn get_gateway_url(&self) -> &str {
        &self.gateway_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pinata_client_creation() {
        let client = PinataClient::new(
            "test_jwt_token".to_string(),
            "https://custom-gateway.com".to_string(),
        );

        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.get_gateway_url(), "https://custom-gateway.com");
    }
}
