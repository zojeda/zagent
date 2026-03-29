use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use whisper_apr::format::{AprV2Writer, TensorDType, build_whisper_metadata};
use whisper_apr::model::ModelConfig;
use whisper_apr::{TranscribeOptions, WhisperApr};

#[derive(Debug, Deserialize)]
pub struct TranscribeRequest {
    pub audio_wav_base64: String,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TranscribeResponse {
    pub text: String,
}

pub async fn transcribe_wav(
    req: TranscribeRequest,
) -> Result<TranscribeResponse, zagent_core::Error> {
    let wav_bytes = STANDARD
        .decode(req.audio_wav_base64.as_bytes())
        .map_err(|e| zagent_core::Error::config(format!("Invalid audio payload: {e}")))?;

    tokio::task::spawn_blocking(move || transcribe_wav_blocking(&wav_bytes, req.language))
        .await
        .map_err(|e| {
            zagent_core::Error::provider("whisper", format!("Transcription task failed: {e}"))
        })?
}

fn transcribe_wav_blocking(
    wav_bytes: &[u8],
    language: Option<String>,
) -> Result<TranscribeResponse, zagent_core::Error> {
    let (mut audio, sample_rate) = decode_wav_to_mono_f32(wav_bytes)?;
    let mut whisper = load_whisper_model()?;
    if sample_rate != 16_000 {
        whisper
            .set_resampler(sample_rate)
            .map_err(map_whisper_error)?;
        audio = whisper.resample(&audio).map_err(map_whisper_error)?;
    }

    let mut options = TranscribeOptions::default();
    options.language = language
        .as_deref()
        .map(str::trim)
        .filter(|lang| !lang.is_empty())
        .map(ToOwned::to_owned);

    let result = whisper
        .transcribe(&audio, options)
        .map_err(map_whisper_error)?;

    Ok(TranscribeResponse {
        text: result.text.trim().to_string(),
    })
}

fn load_whisper_model() -> Result<WhisperApr, zagent_core::Error> {
    if let Some(model_path) = resolve_local_whisper_model_path() {
        match load_whisper_model_from_path(&model_path) {
            Ok(model) => return Ok(model),
            Err(_) => {}
        }
    }

    let generated_path = ensure_generated_tiny_model_path()?;
    load_whisper_model_from_path(&generated_path)
}

fn load_whisper_model_from_path(path: &Path) -> Result<WhisperApr, zagent_core::Error> {
    let model_bytes = std::fs::read(path).map_err(|e| {
        zagent_core::Error::config(format!(
            "Failed reading Whisper model {}: {e}",
            path.display()
        ))
    })?;
    WhisperApr::load_from_apr(&model_bytes).map_err(map_whisper_error)
}

fn ensure_generated_tiny_model_path() -> Result<PathBuf, zagent_core::Error> {
    let cache_dir = whisper_cache_dir()?;
    let apr_path = cache_dir.join("openai-whisper-tiny.apr");
    if apr_path.exists() && load_whisper_model_from_path(&apr_path).is_ok() {
        return Ok(apr_path);
    }

    std::fs::create_dir_all(&cache_dir).map_err(|e| {
        zagent_core::Error::config(format!(
            "Failed creating whisper cache directory {}: {e}",
            cache_dir.display()
        ))
    })?;

    let model_path = cache_dir.join("model.safetensors");
    let vocab_path = cache_dir.join("vocab.json");
    let preprocessor_path = cache_dir.join("preprocessor_config.json");

    download_if_missing(
        "https://huggingface.co/openai/whisper-tiny/resolve/main/model.safetensors",
        &model_path,
    )?;
    download_if_missing(
        "https://huggingface.co/openai/whisper-tiny/resolve/main/vocab.json",
        &vocab_path,
    )?;
    download_if_missing(
        "https://huggingface.co/openai/whisper-tiny/resolve/main/preprocessor_config.json",
        &preprocessor_path,
    )?;

    convert_openai_tiny_to_apr(&model_path, &vocab_path, &preprocessor_path, &apr_path)?;
    Ok(apr_path)
}

fn whisper_cache_dir() -> Result<PathBuf, zagent_core::Error> {
    if let Ok(path) = std::env::var("ZAGENT_WHISPER_CACHE_DIR") {
        let path = path.trim();
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home)
            .join(".cache")
            .join("zagent")
            .join("whisper"));
    }

    Ok(std::env::current_dir()
        .map_err(|e| zagent_core::Error::config(format!("Failed resolving current dir: {e}")))?
        .join(".cache")
        .join("whisper"))
}

fn download_if_missing(url: &str, destination: &Path) -> Result<(), zagent_core::Error> {
    if destination.exists() && destination.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(());
    }

    let client = reqwest::blocking::Client::builder().build().map_err(|e| {
        zagent_core::Error::provider("whisper-apr", format!("HTTP client failed: {e}"))
    })?;
    let response = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            zagent_core::Error::provider("whisper-apr", format!("Download failed for {url}: {e}"))
        })?;
    let bytes = response.bytes().map_err(|e| {
        zagent_core::Error::provider("whisper-apr", format!("Read failed for {url}: {e}"))
    })?;
    std::fs::write(destination, &bytes).map_err(|e| {
        zagent_core::Error::config(format!("Failed writing {}: {e}", destination.display()))
    })
}

fn convert_openai_tiny_to_apr(
    model_path: &Path,
    vocab_path: &Path,
    preprocessor_path: &Path,
    apr_path: &Path,
) -> Result<(), zagent_core::Error> {
    let data = std::fs::read(model_path).map_err(|e| {
        zagent_core::Error::config(format!("Failed reading {}: {e}", model_path.display()))
    })?;
    let tensors = safetensors::SafeTensors::deserialize(&data).map_err(|e| {
        zagent_core::Error::provider("whisper-apr", format!("Invalid safetensors: {e}"))
    })?;

    let metadata = build_whisper_metadata(&ModelConfig::tiny(), "hf://openai/whisper-tiny");
    let mut writer = AprV2Writer::new(metadata);

    let mel = load_mel_filters_from_preprocessor(preprocessor_path)?;
    writer.add_f32_tensor(
        "__mel_filters__",
        vec![mel.n_mels as usize, mel.n_freqs as usize],
        &mel.data,
    );

    let vocab = load_vocabulary_from_json(vocab_path)?;
    let vocab_bytes = vocab.to_bytes();
    writer.add_tensor(
        "__vocab__",
        TensorDType::U8,
        vec![vocab_bytes.len()],
        vocab_bytes,
    );

    for (name, tensor) in tensors.tensors() {
        let Some(f32_data) = convert_tensor_to_f32(&tensor) else {
            continue;
        };
        let shape = tensor.shape().to_vec();
        let mapped_name = map_hf_tensor_name(&name);
        writer.add_f32_tensor(mapped_name, shape, &f32_data);
    }

    let apr_bytes = writer.write().map_err(|e| {
        zagent_core::Error::provider("whisper-apr", format!("APR conversion failed: {e}"))
    })?;
    std::fs::write(apr_path, apr_bytes).map_err(|e| {
        zagent_core::Error::config(format!("Failed writing {}: {e}", apr_path.display()))
    })
}

fn convert_tensor_to_f32(tensor: &safetensors::tensor::TensorView<'_>) -> Option<Vec<f32>> {
    match tensor.dtype() {
        safetensors::Dtype::F32 => Some(
            tensor
                .data()
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        ),
        safetensors::Dtype::F16 => Some(
            tensor
                .data()
                .chunks_exact(2)
                .map(|b| half::f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32())
                .collect(),
        ),
        safetensors::Dtype::BF16 => Some(
            tensor
                .data()
                .chunks_exact(2)
                .map(|b| half::bf16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32())
                .collect(),
        ),
        _ => None,
    }
}

fn map_hf_tensor_name(name: &str) -> String {
    name.strip_prefix("model.").unwrap_or(name).to_string()
}

fn load_vocabulary_from_json(
    vocab_path: &Path,
) -> Result<whisper_apr::tokenizer::Vocabulary, zagent_core::Error> {
    use whisper_apr::tokenizer::{Vocabulary, special_tokens};

    let vocab_json = std::fs::read_to_string(vocab_path).map_err(|e| {
        zagent_core::Error::config(format!("Failed reading {}: {e}", vocab_path.display()))
    })?;
    let token_map: HashMap<String, u32> = serde_json::from_str(&vocab_json)
        .map_err(|e| zagent_core::Error::config(format!("Invalid vocab.json: {e}")))?;

    let mut tokens: Vec<(String, u32)> = token_map.into_iter().collect();
    tokens.sort_by_key(|(_, id)| *id);

    let byte_decoder = build_gpt2_byte_decoder();
    let mut vocab = Vocabulary::new();

    for (token_str, _) in tokens {
        vocab.add_token(decode_gpt2_token(&token_str, &byte_decoder));
    }

    while vocab.len() < special_tokens::SOT as usize {
        vocab.add_token(vec![0]);
    }

    vocab.add_token(b"<|startoftranscript|>".to_vec());
    for lang in [
        "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv",
        "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no",
        "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr",
        "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw",
        "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu",
        "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl",
        "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su",
    ] {
        vocab.add_token(format!("<|{lang}|>").into_bytes());
    }
    vocab.add_token(b"<|translate|>".to_vec());
    vocab.add_token(b"<|transcribe|>".to_vec());
    vocab.add_token(b"<|startoflm|>".to_vec());
    vocab.add_token(b"<|startofprev|>".to_vec());
    vocab.add_token(b"<|nospeech|>".to_vec());
    vocab.add_token(b"<|notimestamps|>".to_vec());
    for i in 0..1501 {
        vocab.add_token(format!("<|{:.2}|>", i as f32 * 0.02).into_bytes());
    }

    Ok(vocab)
}

fn build_gpt2_byte_decoder() -> HashMap<char, u8> {
    let mut decoder = HashMap::new();
    let mut n = 0u32;

    for b in b'!'..=b'~' {
        decoder.insert(char::from(b), b);
    }
    for b in 0xa1u8..=0xac {
        decoder.insert(char::from(b), b);
    }
    for b in 0xaeu8..=0xff {
        decoder.insert(char::from(b), b);
    }
    for b in 0u8..=255 {
        if !decoder.values().any(|&v| v == b) {
            let ch = char::from_u32(256 + n).unwrap_or('?');
            decoder.insert(ch, b);
            n += 1;
        }
    }
    decoder
}

fn decode_gpt2_token(token: &str, decoder: &HashMap<char, u8>) -> Vec<u8> {
    token
        .chars()
        .filter_map(|ch| decoder.get(&ch).copied())
        .collect()
}

fn load_mel_filters_from_preprocessor(
    preprocessor_path: &Path,
) -> Result<whisper_apr::format::MelFilterbankData, zagent_core::Error> {
    let json_str = std::fs::read_to_string(preprocessor_path).map_err(|e| {
        zagent_core::Error::config(format!(
            "Failed reading {}: {e}",
            preprocessor_path.display()
        ))
    })?;
    let config: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| zagent_core::Error::config(format!("Invalid preprocessor config: {e}")))?;
    let mel_filters_value = config.get("mel_filters").ok_or_else(|| {
        zagent_core::Error::config("mel_filters not found in preprocessor_config.json")
    })?;
    let mel_filters_2d: Vec<Vec<f64>> = serde_json::from_value(mel_filters_value.clone())
        .map_err(|e| zagent_core::Error::config(format!("Invalid mel_filters: {e}")))?;
    let n_mels = mel_filters_2d.len();
    let n_freqs = mel_filters_2d.first().map_or(0, Vec::len);
    let data = mel_filters_2d
        .into_iter()
        .flat_map(|row| row.into_iter().map(|v| v as f32))
        .collect();

    Ok(whisper_apr::format::MelFilterbankData {
        n_mels: n_mels as u32,
        n_freqs: n_freqs as u32,
        data,
    })
}

fn decode_wav_to_mono_f32(bytes: &[u8]) -> Result<(Vec<f32>, u32), zagent_core::Error> {
    let cursor = Cursor::new(bytes);
    let mut reader = hound::WavReader::new(cursor)
        .map_err(|e| zagent_core::Error::config(format!("Invalid WAV payload: {e}")))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let sample_rate = spec.sample_rate;

    let interleaved = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|sample| {
                sample
                    .map(|value| value as f32 / i8::MAX as f32)
                    .map_err(invalid_wav_sample)
            })
            .collect::<Result<Vec<_>, _>>()?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|sample| {
                sample
                    .map(|value| value as f32 / i16::MAX as f32)
                    .map_err(invalid_wav_sample)
            })
            .collect::<Result<Vec<_>, _>>()?,
        (hound::SampleFormat::Int, 24 | 32) => reader
            .samples::<i32>()
            .map(|sample| {
                sample
                    .map(|value| value as f32 / i32::MAX as f32)
                    .map_err(invalid_wav_sample)
            })
            .collect::<Result<Vec<_>, _>>()?,
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .map(|sample| sample.map_err(invalid_wav_sample))
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(zagent_core::Error::config(format!(
                "Unsupported WAV format: {:?} {}-bit",
                spec.sample_format, spec.bits_per_sample
            )));
        }
    };

    let audio = if channels == 1 {
        interleaved
    } else {
        interleaved
            .chunks(channels)
            .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
            .collect()
    };

    Ok((audio, sample_rate))
}

fn invalid_wav_sample(error: hound::Error) -> zagent_core::Error {
    zagent_core::Error::config(format!("Invalid WAV sample: {error}"))
}

fn resolve_local_whisper_model_path() -> Option<PathBuf> {
    for key in ["ZAGENT_WHISPER_MODEL", "WHISPER_MODEL"] {
        if let Ok(value) = std::env::var(key) {
            let path = value.trim();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }

    for candidate in [
        "models/tiny.apr",
        "models/base.apr",
        "models/small.apr",
        "models/model.apr",
        "models/whisper-base.apr",
        "tiny.apr",
        "base.apr",
        "small.apr",
        "model.apr",
    ] {
        let path = Path::new(candidate);
        if path.exists() {
            return Some(path.to_path_buf());
        }
    }
    None
}

fn map_whisper_error(error: whisper_apr::WhisperError) -> zagent_core::Error {
    zagent_core::Error::provider("whisper-apr", error.to_string())
}
