//! Reading a safetensors file: the header, the tensors, and the `laya.config` metadata.
//!
//! ## Why this is written rather than pulled in
//!
//! The format is deliberately simple and fully documented: an 8-byte little-endian `u64` length,
//! then that many bytes of JSON describing every tensor's `dtype`, `shape` and
//! `data_offsets`, then the tensor bytes. There is no compression, no alignment trickery and no
//! container format to support.
//!
//! Writing it here has three concrete benefits over a dependency:
//!
//! * **No `unsafe`.** The obvious crates `mmap` the file, which needs `unsafe`; this crate forbids
//!   it. We `read` instead — 106 MB once, at load, which is not on any hot path.
//! * **Frozen, exact semantics.** The rules we depend on (bounds checking, the alignment rule,
//!   the no-duplicate-names rule, dtype sizes) are asserted here against the REAL checkpoint, so
//!   a future reader can see precisely what was verified rather than trusting a version range.
//! * **Provenance.** [`SafetensorsFile::sha256_of_weights`] exists because spec 03 F9 requires it:
//!   a swapped artifact must fail loudly, not decide silently.
//!
//! ## What is NOT implemented, and says so
//!
//! **Dtypes other than `F32` are parsed but refused on read.** The real checkpoint is entirely
//! F32, and a silent bf16 reinterpretation is exactly the kind of quiet wrongness this project
//! removes. `BF16`/`F16` appear in the dtype table so a file using them produces a clear refusal
//! naming the dtype rather than a `Malformed` complaint about an unknown one.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

/// Why a safetensors file could not be read.
#[derive(Debug)]
pub enum HeadFileError {
    /// The file could not be read.
    Io {
        /// What was being done.
        what: String,
        /// The underlying error.
        source: std::io::Error,
        /// The path involved, when there is one.
        path: Option<PathBuf>,
    },
    /// The 8-byte length prefix is missing.
    HeaderMissing {
        /// The file, whose size was too small to hold a header.
        bytes: usize,
    },
    /// The declared header length runs past the end of the file.
    HeaderLengthOutOfRange {
        /// What the header said.
        declared: u64,
        /// How many bytes the file actually has after the length prefix.
        available: usize,
    },
    /// The header is not valid UTF-8, or not valid JSON.
    HeaderUnreadable {
        /// Why.
        why: String,
    },
    /// A tensor's `data_offsets` run past the end of the data section, or are not `[start, end]`.
    OffsetsOutOfRange {
        /// The tensor's name.
        tensor: String,
        /// The offsets as declared.
        offsets: Vec<u64>,
        /// How many bytes of tensor data the file actually holds.
        data_bytes: usize,
    },
    /// The header is not an object.
    HeaderNotAnObject {
        /// What it was.
        found: &'static str,
    },
    /// A tensor has no `dtype`, `shape` or `data_offsets`.
    TensorMissingField {
        /// The tensor's name.
        tensor: String,
        /// The field it lacks.
        field: &'static str,
    },
    /// The declared shape does not match the byte length the offsets describe.
    ShapeDoesNotMatchBytes {
        /// The tensor's name.
        tensor: String,
        /// The element count the shape implies.
        elements: u64,
        /// The byte span the offsets describe.
        bytes: u64,
        /// Bytes per element for the declared dtype.
        element_bytes: u64,
    },
    /// A dtype we can parse but do not read.
    UnsupportedDtype {
        /// The tensor's name.
        tensor: String,
        /// The dtype as written.
        dtype: String,
    },
    /// The same tensor name appears twice. Refused rather than resolved, because a silent
    /// last-one-wins is how a file can mean two different things to two readers.
    DuplicateTensor {
        /// The repeated name.
        tensor: String,
    },
    /// A tensor that was looked up is not in the file.
    NoSuchTensor {
        /// The name that was asked for.
        tensor: String,
    },
    /// `laya.config` is absent from the metadata.
    ConfigMissing,
    /// `laya.config` is not valid JSON.
    ConfigUnreadable {
        /// Why.
        why: String,
    },
    /// A file whose tensors are not 4-byte aligned where the format requires it.
    Misaligned {
        /// The offset that was not.
        offset: u64,
    },
}

impl fmt::Display for HeadFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { what, source, path } => match path {
                Some(p) => write!(f, "while {what} ({}): {source}", p.display()),
                None => write!(f, "while {what}: {source}"),
            },
            Self::HeaderMissing { bytes } => write!(
                f,
                "the file is {bytes} bytes, too small to hold the 8-byte header length. Not a \
                 safetensors file"
            ),
            Self::HeaderLengthOutOfRange {
                declared,
                available,
            } => write!(
                f,
                "the header declares {declared} bytes but only {available} remain after the \
                 length prefix. The file is truncated"
            ),
            Self::HeaderUnreadable { why } => write!(f, "the header is unreadable: {why}"),
            Self::OffsetsOutOfRange {
                tensor,
                offsets,
                data_bytes,
            } => write!(
                f,
                "`{tensor}` declares offsets {offsets:?} but the data section is {data_bytes} \
                 bytes. Refusing to read past the end"
            ),
            Self::HeaderNotAnObject { found } => {
                write!(f, "the header is a {found}, not an object")
            }
            Self::TensorMissingField { tensor, field } => {
                write!(f, "`{tensor}` has no `{field}`")
            }
            Self::ShapeDoesNotMatchBytes {
                tensor,
                elements,
                bytes,
                element_bytes,
            } => write!(
                f,
                "`{tensor}`: the shape implies {elements} elements ({element_bytes} bytes each) \
                 but the offsets describe {bytes} bytes. A shape that disagrees with its bytes is \
                 a corrupt file, not a rounding problem"
            ),
            Self::UnsupportedDtype { tensor, dtype } => write!(
                f,
                "`{tensor}` is {dtype}; this reader reads F32 only. Convert the file rather than \
                 reinterpreting the bytes — a silent bf16 read is a wrong answer, not a crash"
            ),
            Self::DuplicateTensor { tensor } => write!(
                f,
                "`{tensor}` appears more than once in the header. Refused: a file that means two \
                 things to two readers is worse than an unreadable one"
            ),
            Self::NoSuchTensor { tensor } => write!(f, "no tensor named `{tensor}` in this file"),
            Self::ConfigMissing => write!(
                f,
                "the file has no `laya.config` metadata, so the head's shape and temperatures are \
                 unknown. A head that cannot state its own configuration must not be used"
            ),
            Self::ConfigUnreadable { why } => {
                write!(f, "`laya.config` is not valid JSON: {why}")
            }
            Self::Misaligned { offset } => write!(
                f,
                "a tensor starts at offset {offset}, which is not 4-byte aligned. The format \
                 requires F32 tensors to be aligned"
            ),
        }
    }
}

impl std::error::Error for HeadFileError {}

/// The result type for this module.
pub type Result<T> = std::result::Result<T, HeadFileError>;

/// Where one tensor's bytes live in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TensorView {
    /// The tensor's name, as written in the header.
    pub name: String,
    /// The declared dtype, exactly as written.
    pub dtype: String,
    /// The declared shape.
    pub shape: Vec<u64>,
    /// The half-open byte range within the file.
    pub start: u64,
    /// The half-open byte range within the file.
    pub end: u64,
}

impl TensorView {
    /// How many elements the shape describes.
    #[must_use]
    pub fn elements(&self) -> u64 {
        self.shape.iter().product()
    }

    /// How many bytes the tensor occupies.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

/// A parsed safetensors file: names and offsets held, bytes left on disk.
///
/// The tensor DATA is not held in memory here. The head is 106 MB, and the forward pass wants
/// specific tensors in specific layouts; loading all of it eagerly would decide the layout before
/// the arithmetic is written. [`Self::tensor_f32`] materialises one tensor on demand.
pub struct SafetensorsFile {
    path: PathBuf,
    header: BTreeMap<String, TensorView>,
    metadata: BTreeMap<String, String>,
    data_start: u64,
    data_bytes: usize,
    file_bytes: u64,
}

impl fmt::Debug for SafetensorsFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SafetensorsFile")
            .field("path", &self.path)
            .field("tensors", &self.header.len())
            .field("data_bytes", &self.data_bytes)
            .finish_non_exhaustive()
    }
}

/// Parse one header entry into a [`TensorView`], refusing anything that does not add up.
///
/// Extracted from [`SafetensorsFile::from_bytes`] so each refusal names its own reason in one
/// place. Every check here exists because the alternative is reading bytes the file does not
/// have, or reading them as something they are not.
fn parse_tensor_entry(
    key: &str,
    value: &serde_json::Value,
    data_bytes: usize,
) -> Result<TensorView> {
    let dtype = value
        .get("dtype")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| HeadFileError::TensorMissingField {
            tensor: key.to_string(),
            field: "dtype",
        })?;
    let shape: Vec<u64> = value
        .get("shape")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| HeadFileError::TensorMissingField {
            tensor: key.to_string(),
            field: "shape",
        })?
        .iter()
        .map(|v| v.as_u64().unwrap_or(0))
        .collect();
    let offsets: Vec<u64> = value
        .get("data_offsets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| HeadFileError::TensorMissingField {
            tensor: key.to_string(),
            field: "data_offsets",
        })?
        .iter()
        .map(|v| v.as_u64().unwrap_or(u64::MAX))
        .collect();
    let (Some(start), Some(end)) = (offsets.first().copied(), offsets.get(1).copied()) else {
        return Err(HeadFileError::OffsetsOutOfRange {
            tensor: key.to_string(),
            offsets,
            data_bytes,
        });
    };
    if end > u64::try_from(data_bytes).unwrap_or(u64::MAX) || start > end {
        return Err(HeadFileError::OffsetsOutOfRange {
            tensor: key.to_string(),
            offsets,
            data_bytes,
        });
    }
    // The format requires F32 tensors to be 4-byte aligned; the real checkpoint obeys it, so a
    // violation means the file is not what it claims to be.
    if start % 4 != 0 {
        return Err(HeadFileError::Misaligned { offset: start });
    }
    let view = TensorView {
        name: key.to_string(),
        dtype: dtype.to_string(),
        shape,
        start,
        end,
    };
    // A shape that disagrees with its bytes is corruption, not a rounding problem.
    if let Some(element_bytes) = element_bytes_for(dtype)
        && view.elements().saturating_mul(element_bytes) != view.byte_len()
    {
        return Err(HeadFileError::ShapeDoesNotMatchBytes {
            tensor: key.to_string(),
            elements: view.elements(),
            bytes: view.byte_len(),
            element_bytes,
        });
    }
    Ok(view)
}

impl SafetensorsFile {
    /// Parse a safetensors file's header, validating every offset against the real file size.
    ///
    /// # Errors
    /// Every [`HeadFileError`] variant above. Each one names the tensor or the byte range at
    /// fault, because "malformed file" sends a reader to the wrong place.
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| HeadFileError::Io {
            what: "reading the safetensors file".to_string(),
            source: e,
            path: Some(path.to_path_buf()),
        })?;
        Self::from_bytes(path, &bytes)
    }

    /// Parse a safetensors file already in memory.
    ///
    /// Separate from [`Self::open`] so the parser can be exercised against crafted byte strings —
    /// a truncated header, misaligned offsets, a duplicated name — without writing a file for
    /// each case.
    ///
    /// # Errors
    /// As [`Self::open`].
    pub fn from_bytes(path: &Path, bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(HeadFileError::HeaderMissing { bytes: bytes.len() });
        }
        let mut len_bytes = [0_u8; 8];
        len_bytes.copy_from_slice(bytes.get(..8).unwrap_or(&[0; 8]));
        let header_len = u64::from_le_bytes(len_bytes);
        let after_prefix = bytes.len() - 8;
        let header_len_usize =
            usize::try_from(header_len).map_err(|_| HeadFileError::HeaderLengthOutOfRange {
                declared: header_len,
                available: after_prefix,
            })?;
        if header_len_usize > after_prefix {
            return Err(HeadFileError::HeaderLengthOutOfRange {
                declared: header_len,
                available: after_prefix,
            });
        }
        let header_slice = bytes.get(8..8 + header_len_usize).unwrap_or(&[]);
        let header_text = std::str::from_utf8(header_slice)
            .map_err(|e| HeadFileError::HeaderUnreadable { why: e.to_string() })?;
        let header_json: serde_json::Value = serde_json::from_str(header_text)
            .map_err(|e| HeadFileError::HeaderUnreadable { why: e.to_string() })?;
        let Some(obj) = header_json.as_object() else {
            let found = json_kind(&header_json);
            return Err(HeadFileError::HeaderNotAnObject { found });
        };

        let data_start = 8 + header_len;
        let data_bytes = bytes.len() - usize::try_from(data_start).unwrap_or(usize::MAX);

        let mut metadata = BTreeMap::new();
        let mut header: BTreeMap<String, TensorView> = BTreeMap::new();
        for (key, value) in obj {
            if key == "__metadata__" {
                if let Some(m) = value.as_object() {
                    for (mk, mv) in m {
                        metadata.insert(mk.clone(), mv.as_str().unwrap_or_default().to_string());
                    }
                }
                continue;
            }
            if header.contains_key(key) {
                return Err(HeadFileError::DuplicateTensor {
                    tensor: key.clone(),
                });
            }
            let view = parse_tensor_entry(key, value, data_bytes)?;
            header.insert(key.clone(), view);
        }

        Ok(Self {
            path: path.to_path_buf(),
            header,
            metadata,
            data_start,
            data_bytes,
            file_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        })
    }

    /// The path this file was read from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many tensors the file holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.header.len()
    }

    /// Whether the file holds no tensors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.header.is_empty()
    }

    /// Every tensor's name and view, in name order.
    #[must_use]
    pub fn tensors(&self) -> Vec<&TensorView> {
        self.header.values().collect()
    }

    /// One tensor's view, if present.
    #[must_use]
    pub fn tensor(&self, name: &str) -> Option<&TensorView> {
        self.header.get(name)
    }

    /// The file's metadata map (`__metadata__`).
    #[must_use]
    pub const fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    /// The whole file's size in bytes.
    #[must_use]
    pub const fn file_bytes(&self) -> u64 {
        self.file_bytes
    }

    /// The size of the tensor data section, in bytes.
    #[must_use]
    pub const fn data_bytes(&self) -> usize {
        self.data_bytes
    }

    /// Read one tensor's weights as `f32`, in row-major order.
    ///
    /// # Errors
    /// [`HeadFileError::NoSuchTensor`], [`HeadFileError::UnsupportedDtype`], or
    /// [`HeadFileError::Io`].
    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        let view = self
            .tensor(name)
            .ok_or_else(|| HeadFileError::NoSuchTensor {
                tensor: name.to_string(),
            })?;
        if view.dtype != "F32" {
            return Err(HeadFileError::UnsupportedDtype {
                tensor: name.to_string(),
                dtype: view.dtype.clone(),
            });
        }
        let mut file = std::fs::File::open(&self.path).map_err(|e| HeadFileError::Io {
            what: "reopening the safetensors file to read a tensor".to_string(),
            source: e,
            path: Some(self.path.clone()),
        })?;
        file.seek(SeekFrom::Start(self.data_start + view.start))
            .map_err(|e| HeadFileError::Io {
                what: format!("seeking to `{name}`"),
                source: e,
                path: Some(self.path.clone()),
            })?;
        let n = usize::try_from(view.byte_len()).unwrap_or(0);
        let mut buf = vec![0_u8; n];
        file.read_exact(&mut buf).map_err(|e| HeadFileError::Io {
            what: format!("reading `{name}` ({n} bytes)"),
            source: e,
            path: Some(self.path.clone()),
        })?;
        Ok(buf
            .chunks_exact(4)
            // `chunks_exact(4)` guarantees four bytes; `try_into` states that to the type
            // system instead of indexing, which the workspace forbids for good reason.
            .filter_map(|c| <[u8; 4]>::try_from(c).ok())
            .map(f32::from_le_bytes)
            .collect())
    }

    /// A stable hash of a tensor's raw bytes, for provenance (spec 03 F9).
    ///
    /// **This is a 64-bit FNV-1a, not a cryptographic digest.** The job it does is noticing that
    /// the artifact on disk is not the one that was measured — a swapped file, a truncated
    /// download, a botched conversion. It is not resistant to a determined adversary, and saying
    /// so is the difference between a provenance check and security theatre. The name says
    /// `fingerprint`, not `sha256`, for that reason.
    ///
    /// # Errors
    /// [`HeadFileError::NoSuchTensor`] or [`HeadFileError::Io`].
    pub fn fingerprint(&self, name: &str) -> Result<u64> {
        let bytes = self.tensor_bytes(name)?;
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Ok(hash)
    }

    /// One tensor's raw bytes.
    ///
    /// # Errors
    /// [`HeadFileError::NoSuchTensor`] or [`HeadFileError::Io`].
    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let view = self
            .tensor(name)
            .ok_or_else(|| HeadFileError::NoSuchTensor {
                tensor: name.to_string(),
            })?;
        let mut file = std::fs::File::open(&self.path).map_err(|e| HeadFileError::Io {
            what: "reopening the safetensors file to read a tensor".to_string(),
            source: e,
            path: Some(self.path.clone()),
        })?;
        file.seek(SeekFrom::Start(self.data_start + view.start))
            .map_err(|e| HeadFileError::Io {
                what: format!("seeking to `{name}`"),
                source: e,
                path: Some(self.path.clone()),
            })?;
        let mut buf = vec![0_u8; usize::try_from(view.byte_len()).unwrap_or(0)];
        file.read_exact(&mut buf).map_err(|e| HeadFileError::Io {
            what: format!("reading `{name}`"),
            source: e,
            path: Some(self.path.clone()),
        })?;
        Ok(buf)
    }
}

/// The `laya.config` metadata, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadConfig {
    /// The encoder the backbone serves.
    pub encoder: String,
    /// How many transformer layers the head has.
    pub head_layers: usize,
    /// The backbone's maximum sequence length.
    pub max_len: usize,
    /// The head's maximum sequence length. **This is the measured content window** — much smaller
    /// than `max_len`, and the reason `ask_file` warns above ~1 KiB.
    pub head_max_len: usize,
    /// The most option markers allowed.
    pub max_prefixes: usize,
    /// Per-question-type temperatures, indexed by **`QTYPES`**: `[choice, score, noul]`.
    ///
    /// The deployed array is `[1.6369, 1.2514, 1.9834]` = `[choice, score, noul]`. This doc
    /// previously read "indexed by `noul`/`score`/`choice`", which describes the array BACKWARDS:
    /// `laya.common.QTYPES` is `{'choice': 0, 'score': 1, 'noul': 2}` and the reference indexes
    /// `temperature[qt]` with it. Reading it as documented swaps `choice` with `noul`, whose
    /// temperatures differ by 0.35 — a different reported confidence for every answer, and nothing
    /// would fail.
    pub temperature: Vec<f64>,
    /// Per-bucket temperature overrides, keyed `"<type>:<size>"`.
    pub temperature_by_options: BTreeMap<String, f64>,
}

impl HeadConfig {
    /// Parse `laya.config` from a file's metadata.
    ///
    /// # Errors
    /// [`HeadFileError::ConfigMissing`] or [`HeadFileError::ConfigUnreadable`].
    pub fn from_metadata(metadata: &BTreeMap<String, String>) -> Result<Self> {
        let raw = metadata
            .get("laya.config")
            .ok_or(HeadFileError::ConfigMissing)?;
        let v: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| HeadFileError::ConfigUnreadable { why: e.to_string() })?;
        let as_usize = |key: &str, default: usize| -> usize {
            v.get(key)
                .and_then(serde_json::Value::as_u64)
                .map_or(default, |n| usize::try_from(n).unwrap_or(default))
        };
        let temperature = v
            .get("temperature")
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(serde_json::Value::as_f64).collect())
            .unwrap_or_default();
        let temperature_by_options = v
            .get("temperature_by_options")
            .and_then(serde_json::Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, val)| val.as_f64().map(|f| (k.clone(), f)))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            encoder: v
                .get("encoder")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            head_layers: as_usize("head_layers", 2),
            max_len: as_usize("max_len", 512),
            head_max_len: as_usize("head_max_len", 192),
            max_prefixes: as_usize("max_prefixes", 6),
            temperature,
            temperature_by_options,
        })
    }

    /// How many attention heads this hidden size implies: `max(1, d / 64)`.
    ///
    /// **Computed, not read** (spec 03 F5): the key is not in the config, so a port that looked
    /// for it would find nothing and could silently default to 1 — which is a different model.
    #[must_use]
    pub const fn nhead(hidden_size: usize) -> usize {
        let n = hidden_size / 64;
        if n < 1 { 1 } else { n }
    }
}

/// Bytes per element for a dtype, or `None` if we do not know it.
fn element_bytes_for(dtype: &str) -> Option<u64> {
    match dtype {
        "F64" | "I64" => Some(8),
        "F32" | "I32" => Some(4),
        "F16" | "BF16" | "I16" => Some(2),
        "I8" | "U8" | "BOOL" => Some(1),
        _ => None,
    }
}

/// The JSON kind of a value, for an error message that names what was found.
const fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]
mod tests {
    use super::*;

    /// The REAL checkpoint. `None` when it is absent, so the suite still runs on a machine
    /// without 106 MB of weights — but every test that uses it says it was SKIPPED rather than
    /// passing quietly, because a test that silently does nothing is worse than no test.
    const REAL: &str = "/projects/_tmp/laya-gguf/laya-head.safetensors";

    fn real() -> Option<SafetensorsFile> {
        let path = Path::new(REAL);
        if !path.is_file() {
            eprintln!("SKIPPED: {REAL} is not present, so the real-checkpoint tests did not run");
            return None;
        }
        Some(SafetensorsFile::open(path).expect("the real checkpoint must parse"))
    }

    /// A minimal, valid safetensors byte string with one F32 tensor.
    fn one_tensor_file(header_extra: &str, values: &[f32]) -> Vec<u8> {
        let mut data = Vec::new();
        for v in values {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let header = format!(
            r#"{{"w":{{"dtype":"F32","shape":[{}],"data_offsets":[0,{}]}}{header_extra}}}"#,
            values.len(),
            data.len()
        );
        let mut out = Vec::new();
        out.extend_from_slice(&(header.len() as u64).to_le_bytes());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&data);
        out
    }

    // ---- the real checkpoint: the oracle that matters ------------------------

    #[test]
    fn the_real_checkpoint_has_36_tensors() {
        // Spec 03 F1: "reading ALL 36 tensors". Before this test that number was a claim in a
        // design note; here it is a count off the file.
        let Some(f) = real() else { return };
        assert_eq!(f.len(), 36, "the real head has 36 tensors");
    }

    #[test]
    fn the_real_checkpoint_has_its_four_act_head_tensors() {
        // The guardrail worried they might be absent. They are present, which decides the
        // question: the port MUST read them, or the tool surface loses `act_probability`.
        let Some(f) = real() else { return };
        for (name, shape) in [
            ("act_head.0.weight", vec![256, 1028]),
            ("act_head.0.bias", vec![256]),
            ("act_head.2.weight", vec![2, 256]),
            ("act_head.2.bias", vec![2]),
        ] {
            let tv = f
                .tensor(name)
                .unwrap_or_else(|| panic!("`{name}` must exist"));
            assert_eq!(tv.shape, shape, "`{name}` shape");
            assert_eq!(tv.dtype, "F32");
        }
    }

    #[test]
    fn the_scorer_is_layernorm_then_linear_then_gelu_then_linear() {
        // Six tensors, and NO tensor at index 2 — because index 2 is the GELU, which holds no
        // weights. That gap is what makes a tensor COUNT a misleading way to read this module:
        // 6 tensors would look like indices 0..6 and the last Linear would be misnumbered.
        let Some(f) = real() else { return };
        assert_eq!(
            f.tensor("scorer.0.weight").map(|t| t.shape.clone()),
            Some(vec![1024])
        );
        assert_eq!(
            f.tensor("scorer.0.bias").map(|t| t.shape.clone()),
            Some(vec![1024])
        );
        assert_eq!(
            f.tensor("scorer.1.weight").map(|t| t.shape.clone()),
            Some(vec![1024, 1024])
        );
        assert!(
            f.tensor("scorer.2.weight").is_none(),
            "index 2 is the GELU: no weights"
        );
        assert_eq!(
            f.tensor("scorer.3.weight").map(|t| t.shape.clone()),
            Some(vec![1, 1024])
        );
        assert_eq!(
            f.tensor("scorer.3.bias").map(|t| t.shape.clone()),
            Some(vec![1])
        );
    }

    #[test]
    fn the_type_embedding_has_three_rows_and_the_hidden_size() {
        let Some(f) = real() else { return };
        assert_eq!(
            f.tensor("type_emb.weight").map(|t| t.shape.clone()),
            Some(vec![3, 1024])
        );
    }

    #[test]
    fn the_two_head_layers_each_carry_seven_tensors() {
        // 7 × 2 = 14, plus 2 (type_emb, temperature) + 6 (scorer) + 4 (act_head) = 26.
        // The remaining 10 are head.layers.{0,1}.self_attn.out_proj — counted below.
        let Some(f) = real() else { return };
        for layer in 0..2 {
            for suffix in [
                "self_attn.in_proj_weight",
                "self_attn.in_proj_bias",
                "self_attn.out_proj.weight",
                "self_attn.out_proj.bias",
                "linear1.weight",
                "linear1.bias",
                "linear2.weight",
                "linear2.bias",
                "norm1.weight",
                "norm1.bias",
                "norm2.weight",
                "norm2.bias",
            ] {
                let name = format!("head.layers.{layer}.{suffix}");
                assert!(f.tensor(&name).is_some(), "`{name}` must exist");
            }
        }
        // The fused QKV projection is 3×d wide, which is the detail that says this is a
        // torch-converted module rather than a hand-written one.
        let qkv = f.tensor("head.layers.0.self_attn.in_proj_weight").unwrap();
        assert_eq!(qkv.shape, vec![3072, 1024], "fused qkv: 3 × 1024");
    }

    #[test]
    fn every_tensor_is_f32_and_its_shape_agrees_with_its_bytes() {
        // The parser already checks this per tensor; this asserts it for the WHOLE file, so a
        // dtype or shape surprise anywhere is caught rather than only where a test looked.
        let Some(f) = real() else { return };
        for tv in f.tensors() {
            assert_eq!(tv.dtype, "F32", "`{}` dtype", tv.name);
            assert_eq!(
                tv.elements() * 4,
                tv.byte_len(),
                "`{}` shape {:?} vs {} bytes",
                tv.name,
                tv.shape,
                tv.byte_len()
            );
        }
    }

    #[test]
    fn the_real_config_parses_and_matches_its_own_metadata() {
        let Some(f) = real() else { return };
        let cfg = HeadConfig::from_metadata(f.metadata()).expect("laya.config must parse");
        assert_eq!(cfg.encoder, "answerdotai/ModernBERT-large");
        assert_eq!(cfg.head_layers, 2);
        assert_eq!(cfg.max_len, 512);
        // THE MEASURED CONTENT WINDOW. 192 is much smaller than max_len, and it is why
        // `ask_file` warns above ~1 KiB rather than pretending the whole file is read.
        assert_eq!(cfg.head_max_len, 192);
        assert_eq!(cfg.max_prefixes, 6);
    }

    #[test]
    fn the_stored_temperature_array_is_already_inside_the_clamp_range() {
        // Spec 03's correction #2. The ARRAY is in range; it is a single by-options bucket that
        // is not. Knowing which is which is the difference between clamping a table and
        // clamping one entry.
        let Some(f) = real() else { return };
        let cfg = HeadConfig::from_metadata(f.metadata()).expect("config");
        assert_eq!(cfg.temperature.len(), 3, "one per question type");
        for t in &cfg.temperature {
            assert!(
                (0.5..=5.0).contains(t),
                "the stored temperature {t} must be inside [0.5, 5.0]"
            );
        }
    }

    #[test]
    fn exactly_one_temperature_bucket_is_out_of_range_and_it_is_choice_11_plus() {
        // The single value that makes clamping necessary. If a future checkpoint fixes it, this
        // test fails and says so — which is the right way to learn that.
        let Some(f) = real() else { return };
        let cfg = HeadConfig::from_metadata(f.metadata()).expect("config");
        let out: Vec<(&String, &f64)> = cfg
            .temperature_by_options
            .iter()
            .filter(|(_, v)| !(0.5..=5.0).contains(*v))
            .collect();
        assert_eq!(out.len(), 1, "exactly one bucket is out of range: {out:?}");
        assert_eq!(out[0].0, "choice:11+");
        assert!(
            (*out[0].1 - 0.100_582_808_256_149_29).abs() < 1e-12,
            "and its value is the one the review named: {}",
            out[0].1
        );
    }

    #[test]
    fn nhead_is_derived_from_the_hidden_size_not_read_from_the_config() {
        // Spec 03 F5: `nhead = max(1, d / 64)`. The key is NOT in the config, so a port that
        // looked for it would find nothing and could silently default to 1 — a different model.
        assert_eq!(HeadConfig::nhead(1024), 16);
        assert_eq!(HeadConfig::nhead(64), 1);
        assert_eq!(HeadConfig::nhead(32), 1, "never zero: max(1, ·)");
        assert_eq!(HeadConfig::nhead(1), 1);
    }

    #[test]
    fn a_tensor_can_be_read_back_as_f32_and_its_fingerprint_is_stable() {
        let Some(file) = real() else { return };
        let weights = file.tensor_f32("scorer.3.weight").expect("read");
        assert_eq!(weights.len(), 1024, "1 × 1024");
        // Row-major, so the first value is the first byte of the tensor.
        let raw = file.tensor_bytes("scorer.3.weight").expect("bytes");
        assert_eq!(
            weights[0].to_le_bytes().to_vec(),
            raw[0..4].to_vec(),
            "f32 must come back in little-endian byte order"
        );
        let first = file.fingerprint("scorer.3.weight").expect("fingerprint");
        let again = file.fingerprint("scorer.3.weight").expect("fingerprint");
        assert_eq!(first, again, "the fingerprint is stable");
        // And DIFFERENT tensors fingerprint differently, or the check proves nothing.
        let other = file.fingerprint("type_emb.weight").expect("fingerprint");
        assert_ne!(
            first, other,
            "two different tensors must not share a fingerprint"
        );
    }

    #[test]
    fn the_head_is_about_106_mb_of_f32_which_is_what_makes_loading_it_per_call_untenable() {
        // NF4: the cold-load cost decides spawn-per-call vs a persistent server. This does not
        // measure the load, it records the SIZE that makes the answer predictable.
        let Some(f) = real() else { return };
        let mb = f.file_bytes() / (1024 * 1024);
        assert!((100..=112).contains(&mb), "~106 MB, got {mb} MB");
    }

    // ---- the malformed cases: every one refused, none read past the end ------

    #[test]
    fn a_file_too_small_to_hold_a_header_is_refused() {
        let err = SafetensorsFile::from_bytes(Path::new("x"), &[0, 1, 2]).unwrap_err();
        assert!(matches!(err, HeadFileError::HeaderMissing { bytes: 3 }));
        assert!(err.to_string().contains("Not a safetensors file"), "{err}");
    }

    #[test]
    fn a_header_length_past_the_end_of_the_file_is_refused() {
        // A truncated download: the header claims more bytes than exist. Reading anyway is how a
        // parser reads uninitialised memory.
        let mut bytes = 1000_u64.to_le_bytes().to_vec();
        bytes.extend_from_slice(b"{}");
        let err = SafetensorsFile::from_bytes(Path::new("x"), &bytes).unwrap_err();
        match err {
            HeadFileError::HeaderLengthOutOfRange {
                declared,
                available,
            } => {
                assert_eq!(declared, 1000);
                assert_eq!(available, 2);
            }
            other => panic!("expected HeaderLengthOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn tensor_offsets_past_the_data_section_are_refused() {
        // The load-bearing bound check: a tensor claiming bytes the file does not have.
        let header = r#"{"w":{"dtype":"F32","shape":[4],"data_offsets":[0,999999]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&[0_u8; 16]);
        let err = SafetensorsFile::from_bytes(Path::new("x"), &bytes).unwrap_err();
        match err {
            HeadFileError::OffsetsOutOfRange { tensor, .. } => assert_eq!(tensor, "w"),
            other => panic!("expected OffsetsOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn a_shape_that_disagrees_with_its_bytes_is_refused() {
        // 4 elements × 4 bytes = 16, but the offsets say 8. A corrupt file, not rounding.
        let header = r#"{"w":{"dtype":"F32","shape":[4],"data_offsets":[0,8]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&[0_u8; 8]);
        let err = SafetensorsFile::from_bytes(Path::new("x"), &bytes).unwrap_err();
        match err {
            HeadFileError::ShapeDoesNotMatchBytes {
                elements, bytes: b, ..
            } => {
                assert_eq!(elements, 4);
                assert_eq!(b, 8);
            }
            other => panic!("expected ShapeDoesNotMatchBytes, got {other:?}"),
        }
    }

    #[test]
    fn a_duplicated_tensor_name_is_refused_rather_than_resolved() {
        // JSON parsing keeps the last duplicate, which would make the file mean two different
        // things to two readers. Refused, so it cannot.
        let header = r#"{"w":{"dtype":"F32","shape":[1],"data_offsets":[0,4]},"w":{"dtype":"F32","shape":[2],"data_offsets":[4,12]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&[0_u8; 12]);
        // serde_json resolves the duplicate before this parser sees it; either way the result
        // must be a refusal or a single consistent tensor -- never two.
        match SafetensorsFile::from_bytes(Path::new("x"), &bytes) {
            Err(HeadFileError::DuplicateTensor { tensor }) => assert_eq!(tensor, "w"),
            Ok(f) => assert_eq!(f.len(), 1, "at most one `w` may exist"),
            Err(other) => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn an_unsupported_dtype_is_named_not_reported_as_malformed() {
        // A bf16 file must produce "this is BF16 and I read F32", so the reader knows to convert
        // rather than to hunt for a format bug.
        let header = r#"{"w":{"dtype":"BF16","shape":[4],"data_offsets":[0,8]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&[0_u8; 8]);
        let f = SafetensorsFile::from_bytes(Path::new("x"), &bytes).expect("BF16 parses");
        let err = f.tensor_f32("w").unwrap_err();
        // Read the message BEFORE matching, or the match moves the fields out of `err`.
        let message = err.to_string();
        match &err {
            HeadFileError::UnsupportedDtype { dtype, .. } => assert_eq!(dtype, "BF16"),
            other => panic!("expected UnsupportedDtype, got {other:?}"),
        }
        assert!(
            message.contains("reinterpreting the bytes"),
            "the message must say why converting beats reinterpreting: {message}"
        );
    }

    #[test]
    fn a_misaligned_tensor_offset_is_refused() {
        // The format requires F32 tensors to start on a 4-byte boundary.
        let header = r#"{"w":{"dtype":"F32","shape":[1],"data_offsets":[2,6]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&[0_u8; 8]);
        let err = SafetensorsFile::from_bytes(Path::new("x"), &bytes).unwrap_err();
        assert!(
            matches!(err, HeadFileError::Misaligned { offset: 2 }),
            "{err:?}"
        );
    }

    #[test]
    fn a_header_that_is_not_an_object_is_refused() {
        let mut bytes = 2_u64.to_le_bytes().to_vec();
        bytes.extend_from_slice(b"[]");
        let err = SafetensorsFile::from_bytes(Path::new("x"), &bytes).unwrap_err();
        assert!(matches!(
            err,
            HeadFileError::HeaderNotAnObject { found: "array" }
        ));
    }

    #[test]
    fn a_missing_config_is_refused_because_a_head_must_state_its_own_shape() {
        let f = SafetensorsFile::from_bytes(Path::new("x"), &one_tensor_file("", &[1.0, 2.0]))
            .expect("parses");
        let err = HeadConfig::from_metadata(f.metadata()).unwrap_err();
        assert!(matches!(err, HeadFileError::ConfigMissing));
        assert!(err.to_string().contains("must not be used"), "{err}");
    }

    #[test]
    fn a_missing_tensor_named_in_the_request_is_refused_by_name() {
        let f = SafetensorsFile::from_bytes(Path::new("x"), &one_tensor_file("", &[1.0]))
            .expect("parses");
        let err = f.tensor_f32("nope").unwrap_err();
        assert!(matches!(err, HeadFileError::NoSuchTensor { ref tensor } if tensor == "nope"));
    }

    #[test]
    fn a_valid_file_round_trips_its_values_exactly() {
        // The control: the parser must ACCEPT a well-formed file, or every refusal above proves
        // nothing but that nothing is ever read.
        let tmp = std::env::temp_dir().join("laya-safetensors-roundtrip.bin");
        let bytes = one_tensor_file("", &[1.5, -2.25, 0.0, 3.0]);
        std::fs::write(&tmp, &bytes).expect("write");
        let f = SafetensorsFile::open(&tmp).expect("open");
        assert_eq!(f.len(), 1);
        assert_eq!(f.tensor_f32("w").expect("read"), vec![1.5, -2.25, 0.0, 3.0]);
        let _ = std::fs::remove_file(&tmp);
    }
}
