pub struct Lexer<'a> {
    content: &'a [char],
}

impl<'a> Lexer<'a> {
    pub fn new(content: &'a [char]) -> Self {
        Self { content }
    }
    // 跳过开头的空白字符
    fn trim_left(&mut self) {
        while !self.content.is_empty() && self.content[0].is_whitespace() {
            self.content = &self.content[1..];
        }
    }

    // 根据切片位置截取指定长度的字符
    fn chop(&mut self, n: usize) -> &'a [char] {
        let token = &self.content[0..n];
        self.content = &self.content[n..];
        token
    }

    fn chop_while<P>(&mut self, mut predicate: P) -> &'a [char]
    where
        P: FnMut(&char) -> bool,
    {
        let mut n = 0;
        while n < self.content.len() && predicate(&self.content[n]) {
            n += 1;
        }
        self.chop(n)
    }

    // 将连续的数字单独切分为一个Token；将连续的英文字母切分并统一转换为大写(实现不区分大小写检索)
    pub fn next_token(&mut self) -> Option<String> {
        // ex: Rust 2026 rust => Some(RUST) Some(2026) Some(RUST) None
        self.trim_left();
        if self.content.is_empty() {
            return None;
        }

        if self.content[0].is_numeric() {
            return Some(self.chop_while(|x| x.is_numeric()).iter().collect());
        }

        // 使用snowball来做提词，过滤掉一些副词等等去掉词频噪音
        if self.content[0].is_alphabetic() {
            let term = self
                .chop_while(|x| x.is_alphabetic())
                .iter()
                .map(|x| x.to_ascii_uppercase())
                .collect::<String>();
            let mut env = crate::snowball::SnowballEnv::create(&term);
            crate::snowball::algorithms::english_stemmer::stem(&mut env);
            let stemmed_term = env.get_current().to_string();
            return Some(stemmed_term);
        }

        Some(self.chop(1).iter().collect())
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_token()
    }
}
