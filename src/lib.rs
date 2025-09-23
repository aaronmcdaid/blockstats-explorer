use wasm_bindgen::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

macro_rules! console_log {
    ($($t:tt)*) => (log(&format_args!($($t)*).to_string()))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DatasetMetadata {
    pub version: String,
    pub block_range: BlockRange,
    pub datasets: Vec<DatasetInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlockRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetInfo {
    pub name: String,
    pub file: String,
    pub columns: HashMap<String, ColumnInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    #[serde(rename = "type")]
    pub column_type: String,
    pub unit: Option<String>,
    pub description: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct MetricInfo {
    pub name: String,
    pub unit: String,
    pub description: String,
    pub dataset: String,
}

#[derive(Serialize, Deserialize)]
pub struct MetricData {
    pub heights: Vec<u32>,
    pub values: Vec<f64>,
    pub name: String,
    pub unit: String,
}

#[wasm_bindgen]
pub struct FeeExplorer {
    metadata: DatasetMetadata,
}

#[wasm_bindgen]
impl FeeExplorer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> FeeExplorer {
        console_log!("Initializing FeeExplorer WASM module");
        FeeExplorer {
            metadata: DatasetMetadata::default(),
        }
    }

    #[wasm_bindgen]
    pub fn load_metadata(&mut self, metadata_json: &str) -> Result<(), JsValue> {
        console_log!("Loading metadata...");

        self.metadata = serde_json::from_str(metadata_json)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse metadata: {}", e)))?;

        console_log!("Metadata loaded: {} datasets", self.metadata.datasets.len());
        Ok(())
    }

    #[wasm_bindgen]
    pub fn get_available_metrics(&self) -> JsValue {
        let metrics: Vec<MetricInfo> = self.metadata.datasets.iter()
            .flat_map(|ds| {
                ds.columns.iter().filter_map(|(name, col)| {
                    if col.column_type == "metric" {
                        Some(MetricInfo {
                            name: name.clone(),
                            unit: col.unit.clone().unwrap_or_default(),
                            description: col.description.clone().unwrap_or_default(),
                            dataset: ds.name.clone(),
                        })
                    } else {
                        None
                    }
                })
            })
            .collect();

        serde_wasm_bindgen::to_value(&metrics).unwrap()
    }

    #[wasm_bindgen]
    pub fn get_metric_data(&self, metric_names: &JsValue, start_height: u32, end_height: u32) -> Result<JsValue, JsValue> {
        let names: Vec<String> = serde_wasm_bindgen::from_value(metric_names.clone())?;
        console_log!("Getting metric data for: {:?}, range: {}-{}", names, start_height, end_height);

        // For now, return mock data since we don't have real Arrow loading yet
        let mut results = Vec::new();

        for name in names {
            let metric_info = self.metadata.datasets.iter()
                .flat_map(|ds| ds.columns.iter())
                .find(|(col_name, _)| *col_name == &name);

            if let Some((_, col_info)) = metric_info {
                let unit = col_info.unit.clone().unwrap_or_default();

                // Generate mock data
                let num_points = (end_height - start_height + 1).min(1000) as usize;
                let heights: Vec<u32> = (0..num_points)
                    .map(|i| start_height + (i as u32 * (end_height - start_height) / num_points as u32))
                    .collect();
                let values: Vec<f64> = heights.iter()
                    .map(|h| self.generate_mock_value(&name, *h))
                    .collect();

                results.push(MetricData {
                    heights,
                    values,
                    name: name.clone(),
                    unit,
                });
            }
        }

        Ok(serde_wasm_bindgen::to_value(&results).unwrap())
    }

    fn generate_mock_value(&self, metric_name: &str, height: u32) -> f64 {
        let base_height = 700000.0;
        let progress = (height as f64 - base_height) / 100000.0;

        match metric_name {
            "tx_count" => 1500.0 + (progress * 10.0).sin() * 500.0 + (height as f64 % 100.0) * 3.0,
            "fee_avg" => 10.0 + (progress * 5.0).sin() * 8.0 + (height as f64 % 50.0) * 0.1,
            "fee_min" => 1.0 + (height as f64 % 10.0) * 0.2,
            "fee_max" => 100.0 + (progress * 3.0).sin() * 200.0 + (height as f64 % 200.0),
            "block_size" => 1000000.0 + (progress * 7.0).sin() * 300000.0 + (height as f64 % 1000.0) * 100.0,
            "sub_1sat_count" => height as f64 % 50.0,
            "op_return_max_size" => 40.0 + (height as f64 % 40.0),
            "difficulty" => 20000000000000.0 + progress * 10000000000000.0 + (height as f64 % 10000.0) * 1000000000.0,
            _ => height as f64 % 100.0,
        }
    }
}