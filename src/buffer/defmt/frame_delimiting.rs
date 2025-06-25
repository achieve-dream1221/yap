use nom::{
    IResult,
    branch::alt,
    bytes::{
        complete,
        streaming::{self, tag},
    },
    combinator::{cut, map},
    sequence::{preceded, terminated},
};

use crate::buffer::{
    RangeSlice,
    defmt::{DefmtDelimitedSlice, FRAME_END, FRAME_START},
};

#[inline(always)]
pub fn esp_defmt_delimit(input: &[u8]) -> IResult<&[u8], DefmtDelimitedSlice<'_>> {
    const {
        assert!(FRAME_END.len() == 1);
    }

    // Try a framed packet first: 0xFF 0x00 [potentially leading 0x00's] ...content... 0x00
    let packet = map(
        preceded(
            tag(FRAME_START), // header
            cut(terminated(
                preceded(
                    // skip any leading 0x00 bytes after FRAME_START before content
                    complete::take_till(|b| b != FRAME_END[0]),
                    // payload, never empty, must not contain 0x00 mid-data
                    streaming::take_till(|b| b == FRAME_END[0]),
                ),
                tag(FRAME_END), // terminator, 0x00, rzcobs frame end
            )),
        ),
        |inner: &[u8]| {
            let range_slice = unsafe { RangeSlice::from_parent_and_child(input, inner) };
            // Add length of terminating tag,
            let raw_end = range_slice.range.end.wrapping_add(FRAME_END.len());
            // and start from the beginning of the input for the rest.
            DefmtDelimitedSlice::DefmtRzcobs {
                raw: &input[..raw_end],
                inner,
            }
        },
    );

    const FRAME_INIT_BYTE: u8 = FRAME_START[0];

    // If no frame was found (incomplete or otherwise), spit out raw bytes
    // up to (but not including) the next thing that could be the start of a frame.
    let raw_run = map(
        complete::take_till(|b| b == FRAME_INIT_BYTE),
        DefmtDelimitedSlice::Raw,
    );

    // But if we run into a "packet" that matched part of the FRAME_START but not entirely,
    // just return what we have up until the next potential frame.
    let non_defmt_packet = map(
        preceded(
            tag(&[FRAME_INIT_BYTE]),
            complete::take_till(|b| b == FRAME_INIT_BYTE),
        ),
        |raw: &[u8]| {
            let raw_with_ff = &input[0..raw.len() + 1]; // including the 0xFF byte in the result
            DefmtDelimitedSlice::Raw(raw_with_ff)
        },
    );

    alt((packet, raw_run, non_defmt_packet))(input)
}

#[test]
fn esp_defmt_delimit_test() {
    let packet = &[0xFF, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF, 0x00];

    let (rest, delimited) = esp_defmt_delimit(packet).unwrap();

    assert!(rest.is_empty());
    assert_eq!(
        delimited,
        DefmtDelimitedSlice::DefmtRzcobs {
            raw: packet,
            inner: &packet[4..8]
        }
    );

    let packet = &[0xFF, 0x00];
    let result = esp_defmt_delimit(packet);
    assert!(result.is_err());

    let packet = &[0xDE, 0xAD, 0xBE, 0xEF];
    let (rest, delimited) = esp_defmt_delimit(packet).unwrap();
    assert!(rest.is_empty());
    assert_eq!(delimited, DefmtDelimitedSlice::Raw(packet));

    let packet = &[0xDE, 0xAD, 0xBE, 0xEF, 0xFF];
    let (rest, delimited) = esp_defmt_delimit(packet).unwrap();
    assert!(rest.len() == 1);
    assert_eq!(delimited, DefmtDelimitedSlice::Raw(&packet[..4]));
}

// TODO non-esp-println-framed packets
