use std::io;

use bstr::BStr;

use crate::{encode, encode::NL, Kind, Tag, TagRef};

/// An Error used in [`Tag::write_to()`][crate::WriteTo::write_to()].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error("Tags must not start with a dash: '-'")]
    StartsWithDash,
    #[error("The tag name was no valid reference name")]
    InvalidRefName(#[from] git_validate::tag::name::Error),
}

impl From<Error> for io::Error {
    fn from(err: Error) -> Self {
        io::Error::new(io::ErrorKind::Other, err)
    }
}

impl crate::WriteTo for Tag {
    fn write_to(&self, out: impl io::Write) -> io::Result<()> {
        TagRef::from(self).write_to(out)
    }

    fn size(&self) -> usize {
        TagRef::from(self).size()
    }

    fn kind(&self) -> Kind {
        Kind::Tag
    }
}

impl<'a> crate::WriteTo for TagRef<'a> {
    fn write_to(&self, mut out: impl io::Write) -> io::Result<()> {
        encode::trusted_header_id(b"object", &self.target(), &mut out)?;
        encode::trusted_header_field(b"type", self.target_kind.as_bytes(), &mut out)?;
        encode::header_field(b"tag", validated_name(self.name)?, &mut out)?;
        if let Some(tagger) = &self.tagger {
            encode::trusted_header_signature(b"tagger", tagger, &mut out)?;
        }

        out.write_all(NL)?;
        if !self.message.is_empty() {
            out.write_all(self.message)?;
        }
        if let Some(message) = self.pgp_signature {
            out.write_all(NL)?;
            out.write_all(message)?;
        }
        Ok(())
    }

    fn size(&self) -> usize {
        b"object".len() + 1 /* space */ + self.target().kind().len_in_hex() + 1 /* nl */
            + b"type".len() + 1 /* space */ + self.target_kind.as_bytes().len() + 1 /* nl */
            + b"tag".len() + 1 /* space */ + self.name.len() + 1 /* nl */
            + self
                .tagger
                .as_ref()
                .map(|t| b"tagger".len() + 1 /* space */ + t.size() + 1 /* nl */)
                .unwrap_or(0)
            + 1 /* nl */ + self.message.len()
            + self.pgp_signature.as_ref().map(|m| 1 /* nl */ + m.len()).unwrap_or(0)
    }

    fn kind(&self) -> Kind {
        Kind::Tag
    }
}

fn validated_name(name: &BStr) -> Result<&BStr, Error> {
    git_validate::tag::name(name)?;
    if name[0] == b'-' {
        return Err(Error::StartsWithDash);
    }
    Ok(name)
}

#[cfg(test)]
mod tests;
