use toml::value::{Table, Value};

pub(crate) trait TomlExt {
    fn read(&self, key: &str) -> Option<&Value>;
    fn read_mut(&mut self, key: &str) -> Option<&mut Value>;
    fn insert(&mut self, key: &str, value: Value);
    fn delete(&mut self, key: &str) -> Option<Value>;
}

impl TomlExt for Value {
    fn read(&self, key: &str) -> Option<&Value> {
        if let Some((head, tail)) = split(key) {
            self.get(head)?.read(tail)
        } else {
            self.get(key)
        }
    }

    fn read_mut(&mut self, key: &str) -> Option<&mut Value> {
        if let Some((head, tail)) = split(key) {
            self.get_mut(head)?.read_mut(tail)
        } else {
            self.get_mut(key)
        }
    }

    fn insert(&mut self, key: &str, value: Value) {
        if !self.is_table() {
            *self = Value::Table(Table::new());
        }

        let table = self.as_table_mut().expect("unreachable");

        if let Some((head, tail)) = split(key) {
            table
                .entry(head)
                .or_insert_with(|| Value::Table(Table::new()))
                .insert(tail, value);
        } else {
            table.insert(key.to_string(), value);
        }
    }

    fn delete(&mut self, key: &str) -> Option<Value> {
        if let Some((head, tail)) = split(key) {
            self.get_mut(head)?.delete(tail)
        } else if let Some(table) = self.as_table_mut() {
            table.remove(key)
        } else {
            None
        }
    }
}

fn split(key: &str) -> Option<(&str, &str)> {
    let ix = key.find('.')?;

    let (head, tail) = key.split_at(ix);
    // splitting will leave the "."
    let tail = &tail[1..];

    Some((head, tail))
}
