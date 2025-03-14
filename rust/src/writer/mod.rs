//! Abstractions and implementations for writing data to delta tables
// TODO
// - consider file size when writing parquet files
// - handle writer version
#![cfg(all(feature = "arrow", feature = "parquet"))]

pub mod json;
pub mod record_batch;
mod stats;
#[cfg(test)]
pub mod test_utils;
pub mod utils;

use crate::action::{Action, Add, ColumnCountStat, Stats};
use crate::delta::DeltaTable;
use crate::{DeltaDataTypeVersion, DeltaTableError};

use arrow::{datatypes::SchemaRef, datatypes::*, error::ArrowError};
use async_trait::async_trait;
use object_store::Error as ObjectStoreError;
use parquet::{basic::LogicalType, errors::ParquetError};
use serde_json::Value;

pub use json::JsonWriter;
pub use record_batch::RecordBatchWriter;

/// Enum representing an error when calling [`DeltaWriter`].
#[derive(thiserror::Error, Debug)]
pub(crate) enum DeltaWriterError {
    /// Partition column is missing in a record written to delta.
    #[error("Missing partition column: {0}")]
    MissingPartitionColumn(String),

    /// The Arrow RecordBatch schema does not match the expected schema.
    #[error("Arrow RecordBatch schema does not match: RecordBatch schema: {record_batch_schema}, {expected_schema}")]
    SchemaMismatch {
        /// The record batch schema.
        record_batch_schema: SchemaRef,
        /// The schema of the target delta table.
        expected_schema: SchemaRef,
    },

    /// An Arrow RecordBatch could not be created from the JSON buffer.
    #[error("Arrow RecordBatch created from JSON buffer is a None value")]
    EmptyRecordBatch,

    /// A record was written that was not a JSON object.
    #[error("Record {0} is not a JSON object")]
    InvalidRecord(String),

    /// Indicates that a partial write was performed and error records were discarded.
    #[error("Failed to write some values to parquet. Sample error: {sample_error}.")]
    PartialParquetWrite {
        /// Vec of tuples where the first element of each tuple is the skipped value and the second element is the [`ParquetError`] associated with it.
        skipped_values: Vec<(Value, ParquetError)>,
        /// A sample [`ParquetError`] representing the overall partial write.
        sample_error: ParquetError,
    },

    // TODO: derive Debug for Stats in delta-rs
    /// Serialization of delta log statistics failed.
    #[error("Serialization of delta log statistics failed")]
    StatsSerializationFailed {
        /// The stats object that failed serialization.
        stats: Stats,
    },

    /// underlying object store returned an error.
    #[error("ObjectStore interaction failed: {source}")]
    ObjectStore {
        /// The wrapped [`ObjectStoreError`]
        #[from]
        source: ObjectStoreError,
    },

    /// Arrow returned an error.
    #[error("Arrow interaction failed: {source}")]
    Arrow {
        /// The wrapped [`ArrowError`]
        #[from]
        source: ArrowError,
    },

    /// Parquet write failed.
    #[error("Parquet write failed: {source}")]
    Parquet {
        /// The wrapped [`ParquetError`]
        #[from]
        source: ParquetError,
    },

    /// Error returned from std::io
    #[error("std::io::Error: {source}")]
    Io {
        /// The wrapped [`std::io::Error`]
        #[from]
        source: std::io::Error,
    },

    /// Error returned
    #[error(transparent)]
    DeltaTable(#[from] DeltaTableError),
}

impl From<DeltaWriterError> for DeltaTableError {
    fn from(err: DeltaWriterError) -> Self {
        match err {
            DeltaWriterError::Arrow { source } => DeltaTableError::Arrow { source },
            DeltaWriterError::Io { source } => DeltaTableError::Io { source },
            DeltaWriterError::ObjectStore { source } => DeltaTableError::ObjectStore { source },
            DeltaWriterError::Parquet { source } => DeltaTableError::Parquet { source },
            _ => DeltaTableError::Generic(err.to_string()),
        }
    }
}

#[async_trait]
/// Trait for writing data to Delta tables
pub trait DeltaWriter<T> {
    /// write a chunk of values into the internal write buffers.
    async fn write(&mut self, values: T) -> Result<(), DeltaTableError>;

    /// Flush the internal write buffers to files in the delta table folder structure.
    /// The corresponding delta [`Add`] actions are returned and should be committed via a transaction.
    async fn flush(&mut self) -> Result<Vec<Add>, DeltaTableError>;

    /// Flush the internal write buffers to files in the delta table folder structure.
    /// and commit the changes to the Delta log, creating a new table version.
    async fn flush_and_commit(
        &mut self,
        table: &mut DeltaTable,
    ) -> Result<DeltaDataTypeVersion, DeltaTableError> {
        let mut adds = self.flush().await?;
        let mut tx = table.create_transaction(None);
        tx.add_actions(adds.drain(..).map(Action::add).collect());
        let version = tx.commit(None, None).await?;
        Ok(version)
    }
}
