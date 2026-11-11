fn bytes_u32(bytes: &[u8]) -> u32 {
    let mut out = 0u32;
    for &byte in bytes {
        out = (out << 8) | u32::from(byte);
    }
    out
}

/// One EBML element ID (1..=4 octets), preserving its VINT marker bit.
fn element_id(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let first = *bytes.first()?;
    let len = first.leading_zeros() + 1;
    let count = len as usize;
    if first == 0 || count > 4 {
        return None;
    }
    let head = bytes.get(..count)?;
    Some((bytes_u32(head), bytes.get(count..)?))
}

/// One EBML data size (1..=8 octets); indeterminate stream sizes are rejected.
fn data_size(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let first = *bytes.first()?;
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() + 1;
    let count = len as usize;
    let head = bytes.get(..count)?;
    let mask = 1u8.checked_shl(8 - len).unwrap_or(0).wrapping_sub(1);
    let mut raw = u64::from(first & mask);
    for &byte in &head[1..] {
        raw = (raw << 8) | u64::from(byte);
    }
    if raw == (1u64 << (7 * len)) - 1 {
        return None;
    }
    Some((usize::try_from(raw).ok()?, bytes.get(count..)?))
}

fn element(bytes: &[u8]) -> Option<(u32, &[u8], &[u8])> {
    let (id, rest) = element_id(bytes)?;
    let (size, rest) = data_size(rest)?;
    Some((id, rest.get(..size)?, rest.get(size..)?))
}

fn uint32(payload: &[u8]) -> Option<u32> {
    if payload.is_empty() || payload.len() > 4 {
        return None;
    }
    Some(bytes_u32(payload))
}

fn is_webm_header(mut bytes: &[u8]) -> bool {
    let mut webm = false;
    while !bytes.is_empty() {
        let Some((id, payload, rest)) = element(bytes) else {
            return false;
        };
        if id == 0x4282 && payload.split(|&b| b == 0).next() == Some(b"webm") {
            webm = true;
        }
        bytes = rest;
    }
    webm
}

fn video_box(mut bytes: &[u8]) -> Option<(u32, u32)> {
    let (mut width, mut height) = (None, None);
    while !bytes.is_empty() {
        let (id, payload, rest) = element(bytes)?;
        match id {
            0xB0 => width = uint32(payload),
            0xBA => height = uint32(payload),
            _ => {}
        }
        bytes = rest;
    }
    let (w, h) = (width?, height?);
    (w > 0 && h > 0).then_some((w, h))
}

fn track_entry(mut bytes: &[u8]) -> Option<Option<(u32, u32)>> {
    let (mut kind, mut video) = (None, None);
    while !bytes.is_empty() {
        let (id, payload, rest) = element(bytes)?;
        match id {
            0x83 => kind = Some(uint32(payload)?),
            0xE0 => video = Some(video_box(payload)?),
            _ => {}
        }
        bytes = rest;
    }
    Some((kind == Some(1)).then_some(video).flatten())
}

fn tracks_size(mut bytes: &[u8]) -> Option<Option<(u32, u32)>> {
    let mut found = None;
    while !bytes.is_empty() {
        let (id, payload, rest) = element(bytes)?;
        if id == 0xAE
            && let Some(size) = track_entry(payload)?
            && found.is_none()
        {
            found = Some(size);
        }
        bytes = rest;
    }
    Some(found)
}

fn segment_size(mut bytes: &[u8]) -> Option<Option<(u32, u32)>> {
    let mut found = None;
    while !bytes.is_empty() {
        let (id, payload, rest) = element(bytes)?;
        if id == 0x1654_AE6B
            && let Some(size) = tracks_size(payload)?
            && found.is_none()
        {
            found = Some(size);
        }
        bytes = rest;
    }
    Some(found)
}

pub(crate) fn webm_size(mut bytes: &[u8]) -> Option<(u32, u32)> {
    let (id, header, rest) = element(bytes)?;
    if id != 0x1A45_DFA3 || !is_webm_header(header) {
        return None;
    }
    bytes = rest;
    let mut found = None;
    while !bytes.is_empty() {
        let (id, payload, rest) = element(bytes)?;
        if id == 0x1853_8067
            && let Some(size) = segment_size(payload)?
            && found.is_none()
        {
            found = Some(size);
        }
        bytes = rest;
    }
    found
}
