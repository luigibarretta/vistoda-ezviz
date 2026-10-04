//! Bounded SD record search: protocol fallback and V2 continuation paging.

use chrono::NaiveDate;

use super::{
    DeviceSource, DeviceStatus, MAX_RECORDS, MAX_V2_PAGES, RecordList, RecordQuery,
    RecordSearchVersion, SdRecord, V2_PAGE_SIZE, day_window, normalize, parse_common,
    parse_v2_page,
};
use crate::{config::CameraConfig, error::BridgeError};

/// Searches one camera-local day, capability-preferred protocol first.
pub async fn lookup_day(
    source: &dyn DeviceSource,
    camera: &CameraConfig,
    day: NaiveDate,
) -> Result<RecordList, BridgeError> {
    let status = source.device_status(camera).await.unwrap_or_default();
    let order = match status.record_search {
        Some(RecordSearchVersion::Common) => [RecordSearchVersion::Common, RecordSearchVersion::V2],
        _ => [RecordSearchVersion::V2, RecordSearchVersion::Common],
    };
    let mut last_error = None;
    for version in order {
        match search(source, camera, &status, day, version).await {
            Ok(records) => return Ok(RecordList { records }),
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        tracing::warn!(
            error_type = error.diagnostic_code(),
            "SD record search failed"
        );
    }
    Err(BridgeError::UpstreamUnavailable)
}

async fn search(
    source: &dyn DeviceSource,
    camera: &CameraConfig,
    status: &DeviceStatus,
    day: NaiveDate,
    version: RecordSearchVersion,
) -> Result<Vec<SdRecord>, BridgeError> {
    let (start, stop) = day_window(day);
    let mut query = RecordQuery {
        version,
        start,
        stop,
        size: if version == RecordSearchVersion::V2 {
            V2_PAGE_SIZE
        } else {
            MAX_RECORDS
        },
    };
    if version == RecordSearchVersion::Common {
        let value = source.record_page(camera, &query).await?;
        return Ok(normalize(parse_common(&value, status)?));
    }
    let mut records = Vec::new();
    for _ in 0..MAX_V2_PAGES {
        let value = source.record_page(camera, &query).await?;
        let page = parse_v2_page(&value, status)?;
        records.extend(page.records);
        // `isFinished = 0` continuation from the last stop time is inferred
        // from the app's bookkeeping; it runs only while the cursor advances.
        match page.last_end {
            Some(next)
                if !page.finished
                    && next > query.start
                    && next < query.stop
                    && records.len() < MAX_RECORDS =>
            {
                query.start = next;
            }
            _ => break,
        }
    }
    Ok(normalize(records))
}
