//! Simple building-block data that can be read in one go.
//! All are stored in a fixed size (`Sized`) representation.
//! Also, primitives advance the file pointer when read.

use crate::prelude::*;

pub(crate) trait SplitChecked: Sized {
    fn split_checked(&mut self, at: usize) -> Option<Self>;
}
impl<'a> SplitChecked for &'a [u8] {
    fn split_checked(&mut self, at: usize) -> Option<&'a [u8]> {
        if at > self.len() {
            None
        } else {
            let (extracted, remainder) = self.split_at(at);
            *self = remainder;
            Some(extracted)
        }
    }
}

/// Implemented on integer types for reading as big-endian.
pub(crate) trait IntRead: Sized {
    /// Reads a big-endian integer.
    fn read(data: &mut &[u8]) -> StdResult<Self, &'static ErrorKind>;
}
/// Reads the int from u7 bytes, that is, the top bit in all bytes is ignored.
/// For raw reading on integer types, use `read_raw`.
pub(crate) trait IntReadBottom7: Sized {
    /// Read an int from bytes, but only using the bottom 7 bits of each byte.
    fn read_u7(data: &mut &[u8]) -> StdResult<Self, &'static ErrorKind>;
}

/// Implement simple big endian integer reads.
macro_rules! impl_read_int {
    {$( $int:ty ),*} => {
        $(
            impl IntRead for $int {
                fn read(raw: &mut &[u8]) -> StdResult<Self, &'static ErrorKind> {
                    let bytes = raw.split_checked(mem::size_of::<Self>())
                        .ok_or(err_invalid!("failed to read the expected integer"))?;
                    Ok(bytes.iter().fold(0,|mut acc,byte| {
                        acc = acc.checked_shl(8).unwrap_or(0);
                        acc |= *byte as Self;
                        acc
                    }))
                }
            }
        )*
    }
}
impl_read_int! {u8,u16,u32}

/// Slightly restricted integers.
macro_rules! int_feature {
    { $name:ident ; $inner:tt : read_u7 } => {
        impl IntReadBottom7 for $name {
            fn read_u7(raw: &mut &[u8]) -> StdResult<Self, &'static ErrorKind> {
                let bytes = raw.split_checked(mem::size_of::<$inner>())
                    .ok_or(err_invalid!("failed to read the expected integer"))?;
                if cfg!(feature = "strict") {
                    ensure!(bytes.iter().all(|byte| bit_range(*byte, 7..8)==0), err_malformed!("invalid byte with top bit set"));
                }
                let raw = bytes.iter().fold(0, |mut acc,byte| {
                    acc <<= 7;
                    acc |= bit_range(*byte, 0..7) as $inner;
                    acc
                });
                Ok(if cfg!(feature = "strict") {
                    Self::try_from(raw).ok_or(err_malformed!(stringify!("expected " $name ", found " $inner)))?
                } else {
                    //Ignore and truncate extra bits
                    Self::from(raw)
                })
            }
        }
    };
    { $name:ident ; $inner:tt : read } => {
        impl IntRead for $name {
            fn read(raw: &mut &[u8]) -> StdResult<Self, &'static ErrorKind> {
                let raw = $inner::read(raw)?;
                if cfg!(feature = "strict") {
                    Ok(Self::try_from(raw).ok_or(err_malformed!(concat!("expected ", stringify!($name), ", found ", stringify!($inner))))?)
                } else {
                    //Throw away extra bits
                    Ok(Self::from(raw))
                }
            }
        }
    };
}
macro_rules! restricted_int {
    {$(#[$attr:meta])* $name:ident : $inner:tt => $bits:expr ; $( $feature:tt )* } => {
        $(#[$attr])*
        #[derive(Copy, Clone, PartialEq, Eq, Debug)]
        #[allow(non_camel_case_types)]
        pub struct $name($inner);
        impl From<$inner> for $name {
            /// Lossy convertion, loses top bit.
            fn from(raw: $inner) -> Self {
                Self(bit_range(raw, 0..$bits))
            }
        }
        impl Into<$inner> for $name {
            fn into(self) -> $inner {self.0}
        }
        impl $name {
            pub fn try_from(raw: $inner) -> Option<Self> {
                let trunc = bit_range(raw, 0..$bits);
                if trunc == raw {
                    Some($name(trunc))
                } else {
                    None
                }
            }
            pub fn as_int(self) -> $inner { Into::into(self) }
        }
        $( int_feature!{$name ; $inner : $feature} )*
    };
}
restricted_int! {u15: u16 => 15; read}
restricted_int! {u14: u16 => 14; read read_u7}
restricted_int! {u7: u8 => 7; read}
restricted_int! {u4: u8 => 4; read}
restricted_int! {u2: u8 => 2; read}
restricted_int! {u24: u32 => 24;}
impl IntRead for u24 {
    fn read(raw: &mut &[u8]) -> StdResult<u24, &'static ErrorKind> {
        let bytes = raw
            .split_checked(3)
            .ok_or(err_invalid!("failed to read u24 bytes"))?;
        //Using lossy `from` because value is guaranteed to be 24 bits (3 bytes)
        Ok(u24::from(bytes.iter().fold(0, |mut acc, byte| {
            acc <<= 8;
            acc |= *byte as u32;
            acc
        })))
    }
}

restricted_int! {
    /// Referred to in the MIDI spec as "variable length int".
    u28: u32 => 28;
}
impl IntReadBottom7 for u28 {
    fn read_u7(raw: &mut &[u8]) -> StdResult<Self, &'static ErrorKind> {
        let mut int: u32 = 0;
        for _ in 0..4 {
            let byte = match raw.split_checked(1) {
                Some(slice) => slice[0],
                None => {
                    if cfg!(feature = "strict") {
                        bail!(err_malformed!("unexpected eof while reading varlen int"))
                    } else {
                        //Stay with what was read
                        break;
                    }
                }
            };
            int <<= 7;
            int |= bit_range(byte, 0..7) as u32;
            if bit_range(byte, 7..8) == 0 {
                //Since we did at max 4 reads of 7 bits each, there MUST be at max 28 bits in this int
                //Therefore it's safe to call lossy `from`
                return Ok(Self::from(int));
            }
        }
        if cfg!(feature = "strict") {
            Err(err_malformed!("varlen integer larger than 4 bytes"))
        } else {
            //Use the 4 bytes as-is
            Ok(Self::from(int))
        }
    }
}

#[cfg(feature = "std")]
impl u28 {
    pub(crate) fn write_varlen<W: Write>(&self, out: &mut W) -> IoResult<()> {
        let int = self.as_int();
        let mut skipping = true;
        for i in (0..4).rev() {
            let byte = ((int >> (i * 7)) & 0x7F) as u8;
            if skipping && byte == 0 && i != 0 {
                //Skip these leading zeros
            } else {
                //Write down this u7
                skipping = false;
                let byte = if i == 0 {
                    //Last byte
                    byte
                } else {
                    //Leading byte
                    byte | 0x80
                };
                out.write_all(&[byte])?;
            }
        }
        Ok(())
    }
}

/// Reads a slice represented in the input as a `u28` `len` followed by `len` bytes.
pub(crate) fn read_varlen_slice<'a>(raw: &mut &'a [u8]) -> Result<&'a [u8]> {
    let len = u28::read_u7(raw)
        .context(err_invalid!("failed to read varlen slice length"))?
        .as_int();
    Ok(match raw.split_checked(len as usize) {
        Some(slice) => slice,
        None => {
            if cfg!(feature = "strict") {
                bail!(err_malformed!("incomplete varlen slice"))
            } else {
                mem::replace(raw, &[])
            }
        }
    })
}

#[cfg(feature = "std")]
/// Write a slice represented as a varlen `u28` as its length and then the raw bytes.
pub(crate) fn write_varlen_slice<W: Write>(slice: &[u8], out: &mut W) -> IoResult<()> {
    let len = u32::try_from(slice.len())
        .ok()
        .and_then(|len| u28::try_from(len))
        .ok_or_else(|| IoError::new(io::ErrorKind::InvalidInput, "chunk size exceeds 28 bits"))?;
    len.write_varlen(out)?;
    out.write_all(slice)?;
    Ok(())
}

/// The order in which tracks should be laid out when playing back this SMF file.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Format {
    /// This file should have a single track only.
    ///
    /// If the `strict` feature is enabled, an error is raised if the format is
    /// `Format::SingleTrack` and there is not exactly one track.
    SingleTrack,
    /// This file has several tracks that should be played simultaneously.
    ///
    /// Usually the first track controls tempo and other song metadata.
    Parallel,
    /// This file has several tracks, each one a separate song.
    ///
    /// The tracks should be played sequentially, as completely separate MIDI tracks packaged
    /// within a single SMF file.
    Sequential,
}
impl Format {
    pub fn read(raw: &mut &[u8]) -> Result<Self> {
        let format = u16::read(raw)?;
        Ok(match format {
            0 => Self::SingleTrack,
            1 => Self::Parallel,
            2 => Self::Sequential,
            _ => bail!(err_invalid!("invalid smf format")),
        })
    }
    #[cfg(feature = "std")]
    pub fn encode(&self) -> [u8; 2] {
        let code: u16 = match self {
            Self::SingleTrack => 0,
            Self::Parallel => 1,
            Self::Sequential => 2,
        };
        code.to_be_bytes()
    }
}

/// The timing for an SMF file.
/// This can be in ticks/beat or ticks/second.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Timing {
    /// Specifies ticks/beat as a 15-bit integer.
    ///
    /// The length of a beat is not standard, so in order to fully describe the length of a MIDI
    /// tick the [`MetaMessage::Tempo`](enum.MetaMessage.html#Tempo.v) event should be present.
    Metrical(u15),
    /// Specifies ticks/second by dividing a second into frames and then into subframes.
    /// Therefore the length of of a tick is `1/fps/subframe`.
    Timecode(Fps, u8),
}
impl Timing {
    pub fn read(raw: &mut &[u8]) -> Result<Self> {
        let raw =
            u16::read(raw).context(err_invalid!("unexpected eof when reading midi timing"))?;
        if bit_range(raw, 15..16) != 0 {
            //Timecode
            let fps = -(bit_range(raw, 8..16) as i8);
            let subframe = bit_range(raw, 0..8) as u8;
            Ok(Self::Timecode(
                Fps::from_int(fps as u8).ok_or(err_invalid!("invalid smpte fps"))?,
                subframe,
            ))
        } else {
            //Metrical
            Ok(Self::Metrical(u15::from(raw)))
        }
    }
    #[cfg(feature = "std")]
    pub fn encode(&self) -> [u8; 2] {
        match self {
            Self::Metrical(ticksperbeat) => ticksperbeat.as_int().to_be_bytes(),
            Self::Timecode(framespersec, ticksperframe) => {
                [(-(framespersec.as_int() as i8)) as u8, *ticksperframe]
            }
        }
    }
}

/// Encodes an SMPTE time of the day.
///
/// Enforces several guarantees:
///
/// - `hour` is inside [0,23]
/// - `minute` is inside [0,59]
/// - `second` is inside [0,59]
/// - `frame` is inside [0,fps[
/// - `subframe` is inside [0,99]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct SmpteTime {
    hour: u8,
    minute: u8,
    second: u8,
    frame: u8,
    subframe: u8,
    fps: Fps,
}
impl SmpteTime {
    pub fn new(
        hour: u8,
        minute: u8,
        second: u8,
        frame: u8,
        subframe: u8,
        fps: Fps,
    ) -> Option<Self> {
        macro_rules! check {
            ($cond:expr) => {{
                if !{ $cond } {
                    return None;
                }
            }};
        }
        check!(hour < 24);
        check!(minute < 60);
        check!(second < 60);
        check!(frame < fps.as_int());
        check!(subframe < 100);
        Some(Self {
            hour,
            minute,
            second,
            frame,
            subframe,
            fps,
        })
    }
    pub fn hour(&self) -> u8 {
        self.hour
    }
    pub fn minute(&self) -> u8 {
        self.minute
    }
    pub fn second(&self) -> u8 {
        self.second
    }
    pub fn frame(&self) -> u8 {
        self.frame
    }
    pub fn subframe(&self) -> u8 {
        self.subframe
    }
    pub fn fps(&self) -> Fps {
        self.fps
    }
    pub fn second_f32(&self) -> f32 {
        self.second as f32
            + ((self.frame as f32 + self.subframe as f32 / 100.0) / self.fps.as_f32())
    }
    pub fn read(raw: &mut &[u8]) -> Result<Self> {
        let data = raw
            .split_checked(5)
            .ok_or(err_invalid!("failed to read smpte time data"))?;
        let hour_fps = data[0];
        let (hour, fps) = (bit_range(hour_fps, 0..5), bit_range(hour_fps, 5..7));
        let fps = Fps::from_code(u2::from(fps));
        let minute = data[1];
        let second = data[2];
        let frame = data[3];
        let subframe = data[4];
        Ok(Self::new(hour, minute, second, frame, subframe, fps)
            .ok_or(err_invalid!("invalid smpte time"))?)
    }
    #[cfg(feature = "std")]
    pub fn encode(&self) -> [u8; 5] {
        let hour_fps = self.hour() | self.fps().as_code().as_int() << 5;
        [
            hour_fps,
            self.minute(),
            self.second(),
            self.frame(),
            self.subframe(),
        ]
    }
}

/// One of the four FPS values available for SMPTE times, as defined by the MIDI standard.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Fps {
    /// 24 frames per second.
    Fps24,
    /// 25 frames per second.
    Fps25,
    /// Actually `29.97 = 30 / 1.001` frames per second.
    ///
    /// Quite an exotic value because of interesting historical reasons.
    Fps29,
    /// 30 frames per second.
    Fps30,
}
impl Fps {
    /// Does the conversion from a 2-bit fps code to an `Fps` value.
    pub fn from_code(code: u2) -> Self {
        match code.as_int() {
            0 => Self::Fps24,
            1 => Self::Fps25,
            2 => Self::Fps29,
            3 => Self::Fps30,
            _ => unreachable!(),
        }
    }
    /// Does the conversion to a 2-bit fps code.
    pub fn as_code(self) -> u2 {
        u2::from(match self {
            Self::Fps24 => 0,
            Self::Fps25 => 1,
            Self::Fps29 => 2,
            Self::Fps30 => 3,
        })
    }
    /// Converts an integer representing the semantic fps to an `Fps` value (ie. `24` -> `Fps24`).
    pub fn from_int(raw: u8) -> Option<Self> {
        Some(match raw {
            24 => Self::Fps24,
            25 => Self::Fps25,
            29 => Self::Fps29,
            30 => Self::Fps30,
            _ => return None,
        })
    }
    /// Get the integral approximate fps out.
    pub fn as_int(self) -> u8 {
        match self {
            Self::Fps24 => 24,
            Self::Fps25 => 25,
            Self::Fps29 => 29,
            Self::Fps30 => 30,
        }
    }
    /// Get the actual `f32` fps out.
    pub fn as_f32(self) -> f32 {
        match self.as_int() {
            24 => 24.0,
            25 => 25.0,
            29 => 30.0 / 1.001,
            30 => 30.0,
            _ => unreachable!(),
        }
    }
}
