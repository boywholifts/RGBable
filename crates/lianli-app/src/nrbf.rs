//! A reader for .NET's binary serialization format (MS-NRBF), which L-Connect saves its
//! screen themes in. It builds the object graph without running anything from the file.

use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Object(Object),
    Array(Vec<Value>),
    Ref(i32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub class: String,
    pub fields: Vec<(String, Value)>,
}

/// A parsed stream: its first object and every object references point at.
pub struct Graph {
    root: Value,
    objects: HashMap<i32, Value>,
}

/// One value looked at in a graph, following references.
#[derive(Clone, Copy)]
pub struct Node<'a> {
    graph: &'a Graph,
    value: &'a Value,
}

impl Graph {
    pub fn root(&self) -> Node<'_> {
        self.node(&self.root)
    }

    fn node<'a>(&'a self, value: &'a Value) -> Node<'a> {
        let mut value = value;
        for _ in 0..16 {
            match value {
                Value::Ref(id) => match self.objects.get(id) {
                    Some(target) => value = target,
                    None => {
                        return Node {
                            graph: self,
                            value: &Value::Null,
                        }
                    }
                },
                _ => break,
            }
        }
        Node { graph: self, value }
    }
}

/// Field names in these files carry compiler decorations: `<name>k__BackingField`, and a
/// `Base+` prefix for inherited ones.
fn plain_name(name: &str) -> &str {
    let name = name.rsplit('+').next().unwrap_or(name);
    let name = name.strip_suffix("k__BackingField").unwrap_or(name);
    name.trim_start_matches('<').trim_end_matches('>')
}

impl<'a> Node<'a> {
    pub fn get(&self, field: &str) -> Node<'a> {
        if let Value::Object(object) = self.value {
            for (name, value) in &object.fields {
                if plain_name(name) == field {
                    return self.graph.node(value);
                }
            }
        }
        Node {
            graph: self.graph,
            value: &Value::Null,
        }
    }

    pub fn class(&self) -> &'a str {
        match self.value {
            Value::Object(object) => &object.class,
            _ => "",
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self.value, Value::Null)
    }

    pub fn str(&self) -> Option<&'a str> {
        match self.value {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn int(&self) -> Option<i64> {
        match self.value {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i64),
            Value::Object(_) => self.get("value__").int(),
            _ => None,
        }
    }

    pub fn float(&self) -> Option<f64> {
        match self.value {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn bool(&self) -> Option<bool> {
        match self.value {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn bytes(&self) -> Option<&'a [u8]> {
        match self.value {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// The items of an array, or of a .NET `List`/`ObservableCollection`.
    pub fn items(&self) -> Vec<Node<'a>> {
        match self.value {
            Value::Array(items) => items
                .iter()
                .map(|v| self.graph.node(v))
                .filter(|n| !n.is_null())
                .collect(),
            Value::Object(_) => {
                let list = self.get("items");
                let list = if list.is_null() { *self } else { list };
                let size = list.get("_size").int().unwrap_or(i64::MAX).max(0) as usize;
                let mut items = list.get("_items").items();
                items.truncate(size);
                items
            }
            _ => Vec::new(),
        }
    }
}

const MAX_RECORDS: usize = 200_000;
const MAX_LENGTH: usize = 64 << 20;

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .context("theme file ends early")?;
        let bytes = &self.data[self.at..end];
        self.at = end;
        Ok(bytes)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn length(&mut self) -> Result<usize> {
        let mut value = 0usize;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            value |= usize::from(byte & 0x7F) << shift;
            if byte < 0x80 {
                ensure!(value <= MAX_LENGTH, "length {value} is too large");
                return Ok(value);
            }
        }
        bail!("bad length prefix")
    }
    fn string(&mut self) -> Result<String> {
        let n = self.length()?;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.i32()?;
        ensure!((0..=MAX_LENGTH as i32).contains(&n), "bad count {n}");
        Ok(n as usize)
    }

    fn primitive(&mut self, kind: u8) -> Result<Value> {
        Ok(match kind {
            1 => Value::Bool(self.u8()? != 0),
            2 => Value::Int(i64::from(self.u8()?)),
            3 => {
                let first = self.u8()?;
                let extra = match first {
                    0..=0x7F => 0,
                    0xC0..=0xDF => 1,
                    0xE0..=0xEF => 2,
                    _ => 3,
                };
                let mut bytes = vec![first];
                bytes.extend_from_slice(self.take(extra)?);
                Value::Str(String::from_utf8_lossy(&bytes).into_owned())
            }
            5 => Value::Str(self.string()?),
            6 => Value::Float(f64::from_le_bytes(self.take(8)?.try_into()?)),
            7 => Value::Int(i64::from(i16::from_le_bytes(self.take(2)?.try_into()?))),
            8 => Value::Int(i64::from(self.i32()?)),
            9 | 12 | 13 => Value::Int(i64::from_le_bytes(self.take(8)?.try_into()?)),
            10 => Value::Int(i64::from(self.u8()? as i8)),
            11 => Value::Float(f64::from(f32::from_le_bytes(self.take(4)?.try_into()?))),
            14 => Value::Int(i64::from(u16::from_le_bytes(self.take(2)?.try_into()?))),
            15 => Value::Int(i64::from(u32::from_le_bytes(self.take(4)?.try_into()?))),
            16 => Value::Int(u64::from_le_bytes(self.take(8)?.try_into()?) as i64),
            18 => Value::Str(self.string()?),
            other => bail!("unknown primitive type {other}"),
        })
    }
}

#[derive(Clone)]
struct Class {
    name: String,
    members: Vec<String>,
    types: Vec<(u8, Option<u8>)>,
}

struct Parser<'a> {
    reader: Reader<'a>,
    classes: HashMap<i32, Class>,
    objects: HashMap<i32, Value>,
    nulls: usize,
    records: usize,
}

enum Record {
    Value(Value),
    End,
}

impl Parser<'_> {
    fn types(&mut self, count: usize) -> Result<Vec<(u8, Option<u8>)>> {
        let kinds: Vec<u8> = (0..count)
            .map(|_| self.reader.u8())
            .collect::<Result<_>>()?;
        kinds
            .into_iter()
            .map(|kind| {
                Ok(match kind {
                    0 | 7 => (kind, Some(self.reader.u8()?)),
                    3 => {
                        self.reader.string()?;
                        (kind, None)
                    }
                    4 => {
                        self.reader.string()?;
                        self.reader.i32()?;
                        (kind, None)
                    }
                    _ => (kind, None),
                })
            })
            .collect()
    }

    fn member(&mut self, (kind, primitive): (u8, Option<u8>)) -> Result<Value> {
        if kind == 0 {
            return self.reader.primitive(primitive.unwrap_or(0));
        }
        match self.record()? {
            Record::Value(v) => Ok(v),
            Record::End => bail!("theme file ends inside an object"),
        }
    }

    fn object(&mut self, id: i32, class: Class) -> Result<Value> {
        let mut fields = Vec::with_capacity(class.members.len());
        for (name, kind) in class.members.iter().zip(&class.types) {
            fields.push((name.clone(), self.member(*kind)?));
        }
        let object = Value::Object(Object {
            class: class.name.clone(),
            fields,
        });
        self.objects.insert(id, object);
        Ok(Value::Ref(id))
    }

    fn array(&mut self, id: i32, length: usize, kind: (u8, Option<u8>)) -> Result<Value> {
        let mut items = Vec::with_capacity(length.min(4096));
        while items.len() < length {
            items.push(self.member(kind)?);
        }
        self.objects.insert(id, Value::Array(items));
        Ok(Value::Ref(id))
    }

    fn record(&mut self) -> Result<Record> {
        if self.nulls > 0 {
            self.nulls -= 1;
            return Ok(Record::Value(Value::Null));
        }
        self.records += 1;
        ensure!(
            self.records <= MAX_RECORDS,
            "theme file has too many records"
        );
        let kind = self.reader.u8()?;
        Ok(Record::Value(match kind {
            0 => {
                self.reader.take(16)?;
                return self.record();
            }
            12 => {
                self.reader.i32()?;
                self.reader.string()?;
                return self.record();
            }
            4 | 5 => {
                let id = self.reader.i32()?;
                let name = self.reader.string()?;
                let count = self.reader.count()?;
                let members = (0..count)
                    .map(|_| self.reader.string())
                    .collect::<Result<Vec<_>>>()?;
                let types = self.types(count)?;
                if kind == 5 {
                    self.reader.i32()?;
                }
                let class = Class {
                    name,
                    members,
                    types,
                };
                self.classes.insert(id, class.clone());
                self.object(id, class)?
            }
            1 => {
                let id = self.reader.i32()?;
                let metadata = self.reader.i32()?;
                let class = self
                    .classes
                    .get(&metadata)
                    .cloned()
                    .context("unknown class reference")?;
                self.classes.insert(id, class.clone());
                self.object(id, class)?
            }
            2 | 3 => bail!("classes without member types aren't supported"),
            6 => {
                let id = self.reader.i32()?;
                let s = Value::Str(self.reader.string()?);
                self.objects.insert(id, s);
                Value::Ref(id)
            }
            8 => {
                let primitive = self.reader.u8()?;
                self.reader.primitive(primitive)?
            }
            9 => Value::Ref(self.reader.i32()?),
            10 => Value::Null,
            13 => {
                self.nulls = usize::from(self.reader.u8()?).saturating_sub(1);
                Value::Null
            }
            14 => {
                self.nulls = self.reader.count()?.saturating_sub(1);
                Value::Null
            }
            15 => {
                let id = self.reader.i32()?;
                let length = self.reader.count()?;
                let primitive = self.reader.u8()?;
                let value = if primitive == 2 {
                    Value::Bytes(self.reader.take(length)?.to_vec())
                } else {
                    Value::Array(
                        (0..length)
                            .map(|_| self.reader.primitive(primitive))
                            .collect::<Result<_>>()?,
                    )
                };
                self.objects.insert(id, value);
                Value::Ref(id)
            }
            16 | 17 => {
                let id = self.reader.i32()?;
                let length = self.reader.count()?;
                self.array(id, length, (2, None))?
            }
            7 => {
                let id = self.reader.i32()?;
                let shape = self.reader.u8()?;
                let rank = self.reader.count()?;
                ensure!(rank <= 8, "array rank {rank} is too large");
                let mut length = 1usize;
                for _ in 0..rank {
                    length = length.saturating_mul(self.reader.count()?);
                }
                ensure!(length <= MAX_LENGTH, "array is too large");
                if matches!(shape, 3..=5) {
                    for _ in 0..rank {
                        self.reader.i32()?;
                    }
                }
                let kind = self.types(1)?[0];
                self.array(id, length, kind)?
            }
            11 => return Ok(Record::End),
            other => bail!("unknown record type {other} at byte {}", self.reader.at - 1),
        }))
    }
}

pub fn parse(data: &[u8]) -> Result<Graph> {
    let mut parser = Parser {
        reader: Reader { data, at: 0 },
        classes: HashMap::new(),
        objects: HashMap::new(),
        nulls: 0,
        records: 0,
    };
    let mut root = None;
    loop {
        match parser.record()? {
            Record::End => break,
            Record::Value(v) => {
                root.get_or_insert(v);
            }
        }
    }
    Ok(Graph {
        root: root.context("theme file is empty")?,
        objects: parser.objects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(s: &str) -> Vec<u8> {
        let mut out = vec![s.len() as u8];
        out.extend_from_slice(s.as_bytes());
        out
    }

    #[test]
    fn reads_a_class_with_a_string_and_an_int() {
        let mut data = vec![0u8];
        data.extend_from_slice(&[0; 16]);
        data.push(5); // class with members and types
        data.extend_from_slice(&1i32.to_le_bytes());
        data.extend(string("Theme"));
        data.extend_from_slice(&2i32.to_le_bytes());
        data.extend(string("<name>k__BackingField"));
        data.extend(string("<width>k__BackingField"));
        data.extend_from_slice(&[1, 0, 8]); // string, primitive (int32)
        data.extend_from_slice(&2i32.to_le_bytes()); // library
        data.push(6); // the string
        data.extend_from_slice(&3i32.to_le_bytes());
        data.extend(string("68006"));
        data.extend_from_slice(&2288i32.to_le_bytes());
        data.push(11);
        let graph = parse(&data).unwrap();
        let root = graph.root();
        assert_eq!(root.class(), "Theme");
        assert_eq!(root.get("name").str(), Some("68006"));
        assert_eq!(root.get("width").int(), Some(2288));
        assert!(root.get("missing").is_null());
    }

    #[test]
    fn rejects_cut_off_files() {
        assert!(parse(&[0, 1, 2]).is_err());
    }
}
