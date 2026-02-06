//! Sérialisation binaire pour messages registry
//!
//! Fournit sérialisation/désérialisation compacte pour IPC.
//!
//! Format binaire:
//! - Header: 4 bytes (version + type + flags)
//! - Payload: variable selon le type

use alloc::string::String;
use alloc::vec::Vec;

use crate::protocol::{
    RegistryRequest, RegistryResponse, RequestType, ResponseType, RegistryStatsData,
};
use crate::types::{ServiceName, ServiceInfo, ServiceStatus};

/// Version du format de sérialisation
const SERIAL_VERSION: u8 = 1;

/// Erreur de sérialisation
#[derive(Debug, Clone)]
pub enum SerializeError {
    /// Buffer trop petit
    BufferTooSmall,

    /// Version incompatible
    InvalidVersion(u8),

    /// Type invalide
    InvalidType(u16),

    /// Données corrompues
    CorruptedData,

    /// Dépassement de capacité
    Overflow,
}

impl core::fmt::Display for SerializeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BufferTooSmall => write!(f, "buffer too small"),
            Self::InvalidVersion(v) => write!(f, "invalid version: {}", v),
            Self::InvalidType(t) => write!(f, "invalid type: {}", t),
            Self::CorruptedData => write!(f, "corrupted data"),
            Self::Overflow => write!(f, "overflow"),
        }
    }
}

/// Type Result pour sérialisation
pub type SerializeResult<T> = Result<T, SerializeError>;

/// Trait pour sérialisation binaire
pub trait BinarySerialize: Sized {
    /// Sérialise dans un buffer
    fn serialize_into(&self, buf: &mut Vec<u8>) -> SerializeResult<()>;

    /// Désérialise depuis un buffer
    fn deserialize_from(buf: &[u8]) -> SerializeResult<Self>;

    /// Retourne la taille sérialisée (hint)
    fn serialized_size_hint(&self) -> usize {
        64 // Default estimate
    }
}

/// Helper: écrit un u8
#[inline]
fn write_u8(buf: &mut Vec<u8>, value: u8) {
    buf.push(value);
}

/// Helper: écrit un u16 (little-endian)
#[inline]
fn write_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Helper: écrit un u32 (little-endian)
#[inline]
fn write_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Helper: écrit un u64 (little-endian)
#[inline]
fn write_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Helper: écrit une string (length-prefixed)
#[inline]
fn write_string(buf: &mut Vec<u8>, s: &str) {
    let len = s.len() as u16;
    write_u16(buf, len);
    buf.extend_from_slice(s.as_bytes());
}

/// Helper: lit un u8
#[inline]
fn read_u8(buf: &[u8], offset: &mut usize) -> SerializeResult<u8> {
    if *offset >= buf.len() {
        return Err(SerializeError::BufferTooSmall);
    }
    let value = buf[*offset];
    *offset += 1;
    Ok(value)
}

/// Helper: lit un u16
#[inline]
fn read_u16(buf: &[u8], offset: &mut usize) -> SerializeResult<u16> {
    if *offset + 2 > buf.len() {
        return Err(SerializeError::BufferTooSmall);
    }
    let bytes = [buf[*offset], buf[*offset + 1]];
    *offset += 2;
    Ok(u16::from_le_bytes(bytes))
}

/// Helper: lit un u32
#[inline]
fn read_u32(buf: &[u8], offset: &mut usize) -> SerializeResult<u32> {
    if *offset + 4 > buf.len() {
        return Err(SerializeError::BufferTooSmall);
    }
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&buf[*offset..*offset + 4]);
    *offset += 4;
    Ok(u32::from_le_bytes(bytes))
}

/// Helper: lit un u64
#[inline]
fn read_u64(buf: &[u8], offset: &mut usize) -> SerializeResult<u64> {
    if *offset + 8 > buf.len() {
        return Err(SerializeError::BufferTooSmall);
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buf[*offset..*offset + 8]);
    *offset += 8;
    Ok(u64::from_le_bytes(bytes))
}

/// Helper: lit une string
#[inline]
fn read_string(buf: &[u8], offset: &mut usize) -> SerializeResult<String> {
    let len = read_u16(buf, offset)? as usize;
    if *offset + len > buf.len() {
        return Err(SerializeError::BufferTooSmall);
    }
    let bytes = &buf[*offset..*offset + len];
    *offset += len;

    String::from_utf8(bytes.to_vec())
        .map_err(|_| SerializeError::CorruptedData)
}

// Implémentation pour ServiceName
impl BinarySerialize for ServiceName {
    fn serialize_into(&self, buf: &mut Vec<u8>) -> SerializeResult<()> {
        write_string(buf, self.as_str());
        Ok(())
    }

    fn deserialize_from(buf: &[u8]) -> SerializeResult<Self> {
        let mut offset = 0;
        let name = read_string(buf, &mut offset)?;
        ServiceName::new(&name).map_err(|_| SerializeError::CorruptedData)
    }
}

// Implémentation pour ServiceStatus
impl BinarySerialize for ServiceStatus {
    fn serialize_into(&self, buf: &mut Vec<u8>) -> SerializeResult<()> {
        write_u8(buf, *self as u8);
        Ok(())
    }

    fn deserialize_from(buf: &[u8]) -> SerializeResult<Self> {
        let mut offset = 0;
        let value = read_u8(buf, &mut offset)?;

        match value {
            0 => Ok(ServiceStatus::Registering),
            1 => Ok(ServiceStatus::Active),
            2 => Ok(ServiceStatus::Paused),
            3 => Ok(ServiceStatus::Degraded),
            4 => Ok(ServiceStatus::Stopping),
            5 => Ok(ServiceStatus::Stopped),
            6 => Ok(ServiceStatus::Failed),
            _ => Ok(ServiceStatus::Unknown),
        }
    }
}

// Implémentation pour ServiceInfo
impl BinarySerialize for ServiceInfo {
    fn serialize_into(&self, buf: &mut Vec<u8>) -> SerializeResult<()> {
        // Endpoint
        write_string(buf, self.endpoint());

        // Status
        self.status().serialize_into(buf)?;

        // Metadata
        let meta = self.metadata();
        write_u64(buf, meta.registered_at);
        write_u64(buf, meta.last_heartbeat);
        write_u32(buf, meta.version);
        write_u32(buf, meta.failure_count);
        write_u32(buf, meta.flags);

        Ok(())
    }

    fn deserialize_from(buf: &[u8]) -> SerializeResult<Self> {
        let mut offset = 0;

        // Endpoint
        let endpoint = read_string(buf, &mut offset)?;

        // Status
        let status_buf = &buf[offset..];
        let status = ServiceStatus::deserialize_from(status_buf)?;
        offset += 1;

        // Metadata
        let registered_at = read_u64(buf, &mut offset)?;
        let last_heartbeat = read_u64(buf, &mut offset)?;
        let version = read_u32(buf, &mut offset)?;
        let failure_count = read_u32(buf, &mut offset)?;
        let flags = read_u32(buf, &mut offset)?;

        let mut info = ServiceInfo::new(endpoint);
        info.set_status(status);
        let meta = info.metadata_mut();
        meta.registered_at = registered_at;
        meta.last_heartbeat = last_heartbeat;
        meta.version = version;
        meta.failure_count = failure_count;
        meta.flags = flags;

        Ok(info)
    }

    fn serialized_size_hint(&self) -> usize {
        2 + self.endpoint().len() + 1 + 8 + 8 + 4 + 4 + 4
    }
}

// Implémentation pour RegistryRequest
impl BinarySerialize for RegistryRequest {
    fn serialize_into(&self, buf: &mut Vec<u8>) -> SerializeResult<()> {
        // Version + Type
        write_u8(buf, SERIAL_VERSION);
        write_u16(buf, self.request_type.as_u16());

        // Optional fields
        if let Some(ref name) = self.service_name {
            write_u8(buf, 1);
            name.serialize_into(buf)?;
        } else {
            write_u8(buf, 0);
        }

        if let Some(ref info) = self.service_info {
            write_u8(buf, 1);
            info.serialize_into(buf)?;
        } else {
            write_u8(buf, 0);
        }

        if let Some(ref status) = self.status {
            write_u8(buf, 1);
            status.serialize_into(buf)?;
        } else {
            write_u8(buf, 0);
        }

        Ok(())
    }

    fn deserialize_from(buf: &[u8]) -> SerializeResult<Self> {
        let mut offset = 0;

        // Version
        let version = read_u8(buf, &mut offset)?;
        if version != SERIAL_VERSION {
            return Err(SerializeError::InvalidVersion(version));
        }

        // Type
        let type_value = read_u16(buf, &mut offset)?;
        let request_type = RequestType::from_u16(type_value)
            .ok_or(SerializeError::InvalidType(type_value))?;

        // Optional service_name
        let service_name = if read_u8(buf, &mut offset)? == 1 {
            Some(ServiceName::deserialize_from(&buf[offset..])?)
        } else {
            None
        };

        if service_name.is_some() {
            let name_len = read_u16(&buf[offset..], &mut 0)? as usize + 2;
            offset += name_len;
        }

        // Optional service_info
        let service_info = if read_u8(buf, &mut offset)? == 1 {
            let info = ServiceInfo::deserialize_from(&buf[offset..])?;
            let info_size = info.serialized_size_hint();
            offset += info_size;
            Some(info)
        } else {
            None
        };

        // Optional status
        let status = if read_u8(buf, &mut offset)? == 1 {
            Some(ServiceStatus::deserialize_from(&buf[offset..])?)
        } else {
            None
        };

        Ok(RegistryRequest {
            request_type,
            service_name,
            service_info,
            status,
        })
    }
}

// Implémentation pour RegistryResponse
impl BinarySerialize for RegistryResponse {
    fn serialize_into(&self, buf: &mut Vec<u8>) -> SerializeResult<()> {
        // Version + Type
        write_u8(buf, SERIAL_VERSION);
        write_u16(buf, self.response_type.as_u16());

        // Optional service_info
        if let Some(ref info) = self.service_info {
            write_u8(buf, 1);
            info.serialize_into(buf)?;
        } else {
            write_u8(buf, 0);
        }

        // Services list (truncated for inline - full version needs zero-copy)
        write_u16(buf, self.services.len() as u16);
        for (name, info) in &self.services {
            name.serialize_into(buf)?;
            info.serialize_into(buf)?;
        }

        // Error message
        if let Some(ref msg) = self.error_message {
            write_u8(buf, 1);
            write_string(buf, msg);
        } else {
            write_u8(buf, 0);
        }

        // Stats
        if let Some(ref stats) = self.stats {
            write_u8(buf, 1);
            write_u64(buf, stats.total_lookups);
            write_u64(buf, stats.cache_hits);
            write_u64(buf, stats.cache_misses);
            write_u64(buf, stats.bloom_rejections);
            write_u64(buf, stats.total_registrations);
            write_u64(buf, stats.total_unregistrations);
            write_u64(buf, stats.active_services as u64);
        } else {
            write_u8(buf, 0);
        }

        Ok(())
    }

    fn deserialize_from(buf: &[u8]) -> SerializeResult<Self> {
        let mut offset = 0;

        // Version
        let version = read_u8(buf, &mut offset)?;
        if version != SERIAL_VERSION {
            return Err(SerializeError::InvalidVersion(version));
        }

        // Type
        let type_value = read_u16(buf, &mut offset)?;
        let response_type = ResponseType::from_u16(type_value)
            .ok_or(SerializeError::InvalidType(type_value))?;

        // service_info
        let service_info = if read_u8(buf, &mut offset)? == 1 {
            let info = ServiceInfo::deserialize_from(&buf[offset..])?;
            offset += info.serialized_size_hint();
            Some(info)
        } else {
            None
        };

        // services list
        let services_len = read_u16(buf, &mut offset)? as usize;
        let mut services = Vec::with_capacity(services_len);

        for _ in 0..services_len {
            let name = ServiceName::deserialize_from(&buf[offset..])?;
            let name_len = 2 + name.as_str().len();
            offset += name_len;

            let info = ServiceInfo::deserialize_from(&buf[offset..])?;
            offset += info.serialized_size_hint();

            services.push((name, info));
        }

        // error_message
        let error_message = if read_u8(buf, &mut offset)? == 1 {
            Some(read_string(buf, &mut offset)?)
        } else {
            None
        };

        // stats
        let stats = if read_u8(buf, &mut offset)? == 1 {
            let total_lookups = read_u64(buf, &mut offset)?;
            let cache_hits = read_u64(buf, &mut offset)?;
            let cache_misses = read_u64(buf, &mut offset)?;
            let bloom_rejections = read_u64(buf, &mut offset)?;
            let total_registrations = read_u64(buf, &mut offset)?;
            let total_unregistrations = read_u64(buf, &mut offset)?;
            let active_services = read_u64(buf, &mut offset)? as usize;

            Some(RegistryStatsData {
                total_lookups,
                cache_hits,
                cache_misses,
                bloom_rejections,
                total_registrations,
                total_unregistrations,
                active_services,
            })
        } else {
            None
        };

        Ok(RegistryResponse {
            response_type,
            service_info,
            services,
            error_message,            stats,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_name_serialize() {
        let name = ServiceName::new("test_service").unwrap();

        let mut buf = Vec::new();
        name.serialize_into(&mut buf).unwrap();

        let deserialized = ServiceName::deserialize_from(&buf).unwrap();
        assert_eq!(name.as_str(), deserialized.as_str());
    }

    #[test]
    fn test_service_status_serialize() {
        let status = ServiceStatus::Active;

        let mut buf = Vec::new();
        status.serialize_into(&mut buf).unwrap();

        let deserialized = ServiceStatus::deserialize_from(&buf).unwrap();
        assert_eq!(status, deserialized);
    }

    #[test]
    fn test_service_info_serialize() {
        let info = ServiceInfo::new("/tmp/test.sock");

        let mut buf = Vec::new();
        info.serialize_into(&mut buf).unwrap();

        let deserialized = ServiceInfo::deserialize_from(&buf).unwrap();
        assert_eq!(info.endpoint(), deserialized.endpoint());
    }

    #[test]
    fn test_request_serialize() {
        let name = ServiceName::new("test").unwrap();
        let request = RegistryRequest::lookup(name);

        let mut buf = Vec::new();
        request.serialize_into(&mut buf).unwrap();

        let deserialized = RegistryRequest::deserialize_from(&buf).unwrap();
        assert_eq!(request.request_type, deserialized.request_type);
    }

    #[test]
    fn test_response_serialize() {
        let response = RegistryResponse::ok();

        let mut buf = Vec::new();
        response.serialize_into(&mut buf).unwrap();

        let deserialized = RegistryResponse::deserialize_from(&buf).unwrap();
        assert_eq!(response.response_type, deserialized.response_type);
    }
}
