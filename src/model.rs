use super::lexer::Lexer;
use serde::{Deserialize, Serialize};
use sqlite3_sys::sqlite3;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub trait Model {
    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()>;
    fn requires_reindexing(&mut self, path: &Path, last_modified: SystemTime) -> Result<bool, ()>;
    fn add_document(
        &mut self,
        path: PathBuf,
        last_modified: SystemTime,
        content: &[char],
    ) -> Result<(), ()>;
}

pub struct SqliteModel {
    connection: sqlite::Connection,
}

impl SqliteModel {
    fn execute(&self, statement: &str) -> Result<(), ()> {
        self.connection.execute(statement).map_err(|err| {
            eprintln!("ERROR: could not execute query {statement}: {err}");
        })?;
        Ok(())
    }

    pub fn begin(&self) -> Result<(), ()> {
        self.execute("BEGIN;")
    }

    pub fn commit(&self) -> Result<(), ()> {
        self.execute("COMMIT;")
    }

    pub fn open(path: &Path) -> Result<Self, ()> {
        let connection = sqlite::open(path).map_err(|err| {
            eprintln!(
                "Error: could not open sqlite database {path}: {err}",
                path = path.display()
            );
        })?;
        let this = Self { connection };

        this.execute(
            "
            CREATE TABLE IF NOT EXISTS Documents (
                id INTEGER NOT NULL PRIMARY KEY,
                path TEXT,
                term_count INTEGER,
                UNIQUE(path)
            );
        ",
        )?;

        this.execute(
            "
            CREATE TABLE IF NOT EXISTS TermFreq (
                term TEXT,
                doc_id INTEGER,
                freq INTEGER,
                UNIQUE(term, doc_id),
                FOREIGN KEY(doc_id) REFERENCES Documents(id)
            );
       ",
        )?;

        this.execute(
            "
            CREATE TABLE IF NOT EXISTS DocFreq (
                term TEXT,
                freq INTEGER,
                UNIQUE(term)
            );
        ",
        )?;

        Ok(this)
    }
}

impl Model for SqliteModel {
    fn search_query(&self, _query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()> {
        todo!()
    }

    fn requires_reindexing(&mut self, _path: &Path, last_modified: SystemTime) -> Result<bool, ()> {
        Ok(true)
    }

    fn add_document(
        &mut self,
        path: PathBuf,
        _last_modified: SystemTime,
        content: &[char],
    ) -> Result<(), ()> {
        let terms = Lexer::new(content).collect::<Vec<_>>();

        let doc_id = {
            let query = "INSERT INTO Documents (path, term_count) VALUES (:path, :count)";
            let log_err = |err| {
                eprintln!("ERROR: Could not execute query {query}: {err}");
            };
            let mut stmt = self.connection.prepare(query).map_err(log_err)?;

            stmt.bind_iter::<_, (_, sqlite::Value)>([
                (":path", path.display().to_string().as_str().into()),
                (":count", (terms.len() as i64).into()),
            ])
            .map_err(log_err)?;
            stmt.next().map_err(log_err)?;
            unsafe { sqlite3_sys::sqlite3_last_insert_rowid(self.connection.as_raw()) }
        };
        let mut tf = TermFreq::new();
        for term in Lexer::new(content) {
            if let Some(freq) = tf.get_mut(&term) {
                *freq += 1;
            } else {
                tf.insert(term, 1);
            }
        }

        for (term, freq) in &tf {
            // TermFreq
            {
                let query =
                    "INSERT INTO TermFreq(doc_id, term, freq) VALUES (:doc_id, :term, :freq)";
                let log_err = |err| {
                    eprintln!("ERROR: Could not execute query {query}: {err}");
                };
                let mut stmt = self.connection.prepare(query).map_err(log_err)?;
                stmt.bind_iter::<_, (_, sqlite::Value)>([
                    (":doc_id", doc_id.into()),
                    (":term", term.as_str().into()),
                    (":freq", (*freq as i64).into()),
                ])
                .map_err(log_err)?;
                stmt.next().map_err(log_err)?;
            };

            // DocFreq
            {
                let freq = {
                    let query = "SELECT freq FROM DocFreq WHERE term = :term";
                    let log_err = |err| {
                        eprintln!("ERROR: Could not execute query {query}: {err}");
                    };
                    let mut stmt = self.connection.prepare(query).map_err(log_err)?;
                    stmt.bind_iter::<_, (_, sqlite::Value)>([(":term", term.as_str().into())])
                        .map_err(log_err)?;
                    match stmt.next().map_err(log_err)? {
                        sqlite::State::Row => stmt.read::<i64, _>("freq").map_err(log_err)?,
                        sqlite::State::Done => 0,
                    }
                };

                let query = "INSERT OR REPLACE INTO DocFreq(term,freq) VALUES (:term, :freq)";
                let log_err = |err| eprintln!("ERROR: Could not execute query {query}: {err}");
                let mut stmt = self.connection.prepare(query).map_err(log_err)?;
                stmt.bind_iter::<_, (_, sqlite::Value)>([
                    (":term", term.as_str().into()),
                    (":freq", (freq + 1).into()),
                ])
                .map_err(log_err)?;
                stmt.next().map_err(log_err)?;
            }
        }

        Ok(())
    }
}

// 文档频率 记录某个单词在多少个文件中出现过 不管一个词在文件 A 里出现 100 次还是 1 次，对 DocFreq 来说都只算作“在 1 个文件中出现过”
pub type DocFreq = HashMap<String, usize>;
// 词频 记录某个单词在当前文件中出现的次数
pub type TermFreq = HashMap<String, usize>;

#[derive(Deserialize, Serialize)]
pub struct Doc {
    tf: TermFreq,
    count: usize,
    last_modified: SystemTime,
}

type Docs = HashMap<PathBuf, Doc>;

#[derive(Default, Deserialize, Serialize)]
pub struct InMemoryModel {
    pub docs: Docs,
    df: DocFreq,
}

impl InMemoryModel {
    fn remove_document(&mut self, file_path: &Path) {
        if let Some(doc) = self.docs.remove(file_path) {
            for t in doc.tf.keys() {
                if let Some(f) = self.df.get_mut(t) {
                    *f -= 1;
                }
            }
        }
    }
}

impl Model for InMemoryModel {
    fn requires_reindexing(
        &mut self,
        file_path: &Path,
        last_modified: SystemTime,
    ) -> Result<bool, ()> {
        if let Some(doc) = self.docs.get(file_path) {
            return Ok(doc.last_modified < last_modified);
        }
        return Ok(true);
    }

    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()> {
        let mut result = Vec::new();
        let tokens = Lexer::new(&query).collect::<Vec<_>>();
        for (path, doc) in &self.docs {
            let mut rank = 0f32;
            for token in &tokens {
                rank += compute_tf(token, doc) * compute_idf(&token, self.docs.len(), &self.df);
            }
            result.push((path.clone(), rank));
        }
        result.sort_by(|(_, rank1), (_, rank2)| rank1.partial_cmp(rank2).unwrap());
        result.reverse();
        Ok(result)
    }

    fn add_document(
        &mut self,
        file_path: PathBuf,
        last_modified: SystemTime,
        content: &[char],
    ) -> Result<(), ()> {
        self.remove_document(&file_path);
        let mut tf = TermFreq::new();

        let mut count = 0;
        for t in Lexer::new(content) {
            if let Some(f) = tf.get_mut(&t) {
                *f += 1;
            } else {
                tf.insert(t, 1);
            }
            count += 1;
        }

        for t in tf.keys() {
            if let Some(f) = self.df.get_mut(t) {
                *f += 1;
            } else {
                self.df.insert(t.to_string(), 1);
            }
        }

        self.docs.insert(
            file_path,
            Doc {
                count,
                tf,
                last_modified,
            },
        );
        Ok(())
    }
}

// https://zh.wikipedia.org/wiki/Tf-idfhttps://zh.wikipedia.org/wiki/Tf-idf
// tfidf
// 某一特定文件内的高词语频率，以及该词语在整个文件集合中的低文件频率，可以产生出高权重的tfidf,因此tfidf倾向于过滤常见的词语
// tf = 该词出现的次数 / 当前文件中所有词的总数
fn compute_tf(t: &str, doc: &Doc) -> f32 {
    let n = doc.count as f32;
    let m = doc.tf.get(t).cloned().unwrap_or(0) as f32;
    m / n
}

// idf = lo个0（文件总数 / 包含该词的文件数）
fn compute_idf(t: &str, n: usize, df: &DocFreq) -> f32 {
    let n = n as f32;
    let m = df.get(t).cloned().unwrap_or(1) as f32;
    (n / m).log10()
}
