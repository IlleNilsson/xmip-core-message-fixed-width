#![forbid(unsafe_code)]

//! Fixed-width: lines of positioned fields, the kind a mainframe or a bank
//! writes, where a field is known by where it lies and not by any separator.
//! Without positions, one part per line, named by its number in the content,
//! with the line's bytes as written and without its terminator. With the
//! positions a caller names — `start` bytes in, `width` bytes wide — each
//! line is cut into them, one part per position, named `<line>.<position>`.
//! No type is announced, because a fixed-width file says nothing about
//! itself (ADR-0047).
//!
//! The lines and the cut are the Foundation's [`record`] walk. Nothing is
//! decoded, trimmed or typed: a shape sections and names, a contract
//! validates. What cannot be sectioned — a line shorter than its positions,
//! bytes that are not text — is refused with the byte where the walk
//! stopped. The shape claims `text/x-fixed-width` and, asked without a media
//! type, recognises at least two lines of one length, or, with positions,
//! lines that all fill them.

use message::record::{self, Position};
use message::{Part, Shape, ShapeError, Shaped};
use stream::Stream;

/// The fixed-width shape.
#[derive(Clone, Debug, Default)]
pub struct FixedWidth {
    positions: Vec<Position>,
}

impl FixedWidth {
    /// One part per line, named by its number.
    #[must_use]
    pub const fn lines() -> Self {
        Self {
            positions: Vec::new(),
        }
    }

    /// One part per named position on each line.
    #[must_use]
    pub fn positioned(positions: impl Into<Vec<Position>>) -> Self {
        Self {
            positions: positions.into(),
        }
    }

    /// The bytes a line must have to fill every position.
    fn needed(&self) -> usize {
        self.positions.iter().map(Position::end).max().unwrap_or(0)
    }
}

impl Shape for FixedWidth {
    fn technology(&self) -> &'static str {
        "fixed-width"
    }

    fn media_types(&self) -> &'static [&'static str] {
        &["text/x-fixed-width"]
    }

    fn recognises(&self, bytes: &[u8]) -> bool {
        if record::text(bytes).is_err() {
            return false;
        }
        let lines = record::lines(bytes);
        if self.positions.is_empty() {
            lines.len() >= 2 && lines.iter().all(|line| line.len() == lines[0].len())
        } else {
            !lines.is_empty() && lines.iter().all(|line| line.len() >= self.needed())
        }
    }

    fn shape(&self, stream: &Stream) -> Result<Shaped, ShapeError> {
        let bytes = stream.bytes();
        let refused = |stop| ShapeError::refused("fixed-width", stop);
        record::text(bytes).map_err(refused)?;
        let lines = record::lines(bytes);
        if lines.is_empty() {
            return Err(refused(("no lines", 0)));
        }
        let media = stream
            .media_type()
            .map_or_else(|| "text/plain".to_string(), str::to_string);
        let mut parts = Vec::with_capacity(lines.len());
        for (index, line) in lines.into_iter().enumerate() {
            let number = index + 1;
            if self.positions.is_empty() {
                let part = Part::new(Some(number.to_string()), &bytes[line], Some(media.clone()));
                parts.push(part);
                continue;
            }
            let cut = record::positioned(&bytes[line.clone()], line.start, &self.positions)
                .map_err(refused)?;
            parts.extend(cut.into_iter().map(|(name, field)| {
                Part::new(Some(format!("{number}.{name}")), field, Some(media.clone()))
            }));
        }
        Ok(Shaped {
            parts,
            message_type: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcore::StreamId;

    fn stream(bytes: &[u8], media: Option<&str>) -> Stream {
        Stream::new(StreamId::new(1), bytes.to_vec(), media.map(str::to_string))
    }

    fn layout() -> FixedWidth {
        FixedWidth::positioned([Position::new("id", 0, 3), Position::new("name", 3, 4)])
    }

    #[test]
    fn without_positions_every_line_is_a_numbered_part_without_its_terminator() {
        let shaped = FixedWidth::lines()
            .shape(&stream(b"001Anna\r\n002Bo  \n", None))
            .expect("lines");
        assert_eq!(shaped.parts.len(), 2);
        assert_eq!(shaped.parts[0].name.as_deref(), Some("1"));
        assert_eq!(shaped.parts[0].bytes, b"001Anna");
        assert_eq!(shaped.parts[0].media_type.as_deref(), Some("text/plain"));
        assert_eq!(shaped.parts[1].name.as_deref(), Some("2"));
        assert_eq!(shaped.parts[1].bytes, b"002Bo  ");
        assert_eq!(shaped.message_type, None);

        let kept = FixedWidth::default()
            .shape(&stream(b"x", Some("text/x-fixed-width; charset=utf-8")))
            .expect("one line");
        assert_eq!(
            kept.parts[0].media_type.as_deref(),
            Some("text/x-fixed-width; charset=utf-8")
        );
    }

    #[test]
    fn with_positions_every_line_is_cut_into_parts_named_by_line_and_position() {
        let shaped = layout()
            .shape(&stream(b"001Anna\n002Bo  extra", None))
            .expect("fits");
        assert_eq!(shaped.parts.len(), 4);
        assert_eq!(shaped.parts[0].name.as_deref(), Some("1.id"));
        assert_eq!(shaped.parts[0].bytes, b"001");
        assert_eq!(shaped.parts[1].name.as_deref(), Some("1.name"));
        assert_eq!(shaped.parts[1].bytes, b"Anna");
        assert_eq!(shaped.parts[2].name.as_deref(), Some("2.id"));
        assert_eq!(shaped.parts[3].name.as_deref(), Some("2.name"));
        assert_eq!(shaped.parts[3].bytes, b"Bo  ");
    }

    #[test]
    fn a_short_line_bytes_that_are_not_text_and_no_lines_at_all_are_refused() {
        let short = layout()
            .shape(&stream(b"001Anna\n002Bo", None))
            .expect_err("second line is short");
        assert_eq!(short.offset, Some(13));
        assert_eq!(
            short.to_string(),
            "fixed-width: the line is shorter than its positions at byte 13"
        );

        let binary = FixedWidth::lines()
            .shape(&stream(b"ab\xfe", None))
            .expect_err("not text");
        assert_eq!(binary.reason, "not UTF-8");
        assert_eq!(binary.offset, Some(2));

        let empty = FixedWidth::lines()
            .shape(&stream(b"", None))
            .expect_err("nothing");
        assert_eq!(empty.reason, "no lines");
        assert_eq!(empty.offset, Some(0));
    }

    #[test]
    fn the_shape_claims_fixed_width_and_recognises_lines_of_one_length_or_of_its_positions() {
        assert_eq!(FixedWidth::lines().technology(), "fixed-width");
        assert_eq!(FixedWidth::lines().media_types(), &["text/x-fixed-width"]);
        assert!(FixedWidth::lines().recognises(b"001Anna\n002Bo  \n"));
        assert!(!FixedWidth::lines().recognises(b"001Anna\n002Bo\n"));
        assert!(!FixedWidth::lines().recognises(b"001Anna"));
        assert!(!FixedWidth::lines().recognises(b"ab\nab\x00"));
        assert!(layout().recognises(b"001Anna"));
        assert!(layout().recognises(b"001Anna\n002Bo  longer"));
        assert!(!layout().recognises(b"001An"));
        assert!(!layout().recognises(b""));

        let shapes: [&dyn Shape; 1] = [&FixedWidth::lines()];
        let by_media = message::choose(&shapes, &stream(b"x", Some("Text/X-Fixed-Width")));
        assert_eq!(by_media.map(Shape::technology), Some("fixed-width"));
        let by_look = message::choose(&shapes, &stream(b"aa\nbb", None));
        assert_eq!(by_look.map(Shape::technology), Some("fixed-width"));
        assert!(message::choose(&shapes, &stream(b"a\nbb", None)).is_none());
    }
}
