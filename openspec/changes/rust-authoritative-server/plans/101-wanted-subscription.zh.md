# R4 会话声明视距与 wanted 订阅

英文同名计划为精确契约。基线e1480d85保留已验证passive；R4证据585e28d另列。默认上限33，会话声明2..64保存；半径min(声明+1,上限)，0为中心一列，最大65。ServerLimits保留六参数构造器，新增独立视界builder/getter；不使用active或出生恢复半径。所有发布与resync读取同一会话wanted。旧视界夹具明确cap2，新夹具验证真实default33与上/下界、隔离、Ready、移动forget、watermark，实际Memory/TCP握手与有序字节。宿主先R4 RED再执行全部Cargo GREEN，Claude仅六Rust路径。静态review与宿主runtime分开；最多4调用、两轮、budget null、dev600/review300与有界窗口，冻结Loom不修改。无chat、schema、binary扩展，无push/merge/deploy，无3.7整体验收。架构skill不变。
