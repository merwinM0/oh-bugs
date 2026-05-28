use regex::Regex;

/// 输出监视器，检测输出中的错误关键字
pub struct Watcher {
    patterns: Vec<Regex>,
}

impl Watcher {
    /// 从关键字列表构建监视器（大小写不敏感）
    pub fn new(keywords: &[String]) -> anyhow::Result<Self> {
        let patterns: Vec<Regex> = keywords
            .iter()
            .map(|kw| {
                let escaped = regex::escape(kw);
                Regex::new(&format!("(?i){}", escaped))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { patterns })
    }

    /// 扫描 byte 切片中的错误关键字，返回匹配次数
    pub fn scan(&self, data: &[u8]) -> usize {
        let text = String::from_utf8_lossy(data);
        let mut count = 0;
        for pattern in &self.patterns {
            count += pattern.find_iter(&text).count();
        }
        count
    }


}
