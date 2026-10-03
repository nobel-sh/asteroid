use std::{
    collections::btree_map,
    iter::Peekable,
    ops::{Bound, RangeBounds},
};

use crate::{
    entry::{Entry, StoredValue},
    memtable::Memtable,
    table::{SSTable, TableIterator},
    Key, Result, Value,
};

/// A forward scan yielding the newest live value for each key, then stopping on error.
pub struct DbIterator<'a> {
    memtable: Peekable<btree_map::Range<'a, Key, Entry>>,
    tables: Vec<Peekable<TableIterator<'a>>>,
    done: bool,
}

impl<'a> DbIterator<'a> {
    pub(crate) fn new(
        memtable: &'a Memtable,
        tables: &'a [SSTable],
        range: impl RangeBounds<Key>,
    ) -> Self {
        let start = range.start_bound().cloned();
        let end = range.end_bound().cloned();
        let done = match (&start, &end) {
            (Bound::Included(a), Bound::Included(b)) => a > b,
            (Bound::Included(a) | Bound::Excluded(a), Bound::Included(b) | Bound::Excluded(b)) => {
                a >= b
            }
            _ => false,
        };
        Self {
            memtable: if done {
                memtable.range(Bound::Unbounded, Bound::Unbounded)
            } else {
                memtable.range(start.clone(), end.clone())
            }
            .peekable(),
            tables: tables
                .iter()
                .filter(|table| !done && table.overlaps(start.as_ref(), end.as_ref()))
                .map(|table| TableIterator::from_bounds(table, &start, end.clone()).peekable())
                .collect(),
            done,
        }
    }
}

impl Iterator for DbIterator<'_> {
    type Item = Result<(Key, Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            let mut key = self.memtable.peek().map(|(key, _)| *key);
            for table in &mut self.tables {
                match table.peek() {
                    Some(Err(_)) => {
                        self.done = true;
                        return Some(Err(table.next().unwrap().unwrap_err()));
                    }
                    Some(Ok((candidate, _))) if key.is_none_or(|key| candidate < key) => {
                        key = Some(candidate);
                    }
                    _ => {}
                }
            }
            let Some(key) = key.cloned() else {
                self.done = true;
                return None;
            };
            let mut newest = None;
            if self
                .memtable
                .peek()
                .is_some_and(|(candidate, _)| **candidate == key)
            {
                newest = self.memtable.next().map(|(_, entry)| entry.clone());
            }
            for table in &mut self.tables {
                if table
                    .peek()
                    .is_some_and(|entry| matches!(entry, Ok((candidate, _)) if candidate == &key))
                {
                    let (_, entry) = table.next().unwrap().unwrap();
                    if newest
                        .as_ref()
                        .is_none_or(|newest: &Entry| entry.sequence > newest.sequence)
                    {
                        newest = Some(entry);
                    }
                }
            }
            if let Some(Entry {
                value: StoredValue::Data(value),
                ..
            }) = newest
            {
                return Some(Ok((key, value)));
            }
        }
        None
    }
}

impl std::iter::FusedIterator for DbIterator<'_> {}
