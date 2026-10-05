# 伙伴待恢复 producer 实施计划

同步英文权威计划：108-source-companion-restore.md。原4.1补齐已存在 companion scan 的实际注册、保留 wanted、Ready 后激活及同 tick Native 消费；不引入 Go 明确排除的主动脱困/越界重置。沿用干净6193及已有隔离树。

主 Agent 冻结：最多4个伙伴、radius16/1089列/9spawn chunks/每saved候选最多4chunk、pending union最多36key（候选阶段最多4restore+anchor，拒绝候选先清restore再进入最多9spawn；completed贡献0，Companion不restart，因此4×9）；startup checked注册仅Overworld，先拒绝body ID不符或原始body.dimension!=0，再用project_companion校验（helper会从actor覆盖dimension，不能用于悄悄归一化坏输入）、全量背包/look保真、常量 Actor+Runtime compound；Acquire后pending companion先于player扫描，激活 tick忽略此前Pending动作但照常中性Native物理。scan由authority独占、tick借出归还（包括失败），完成scan不重放，reset按source发布阶段消费。

三内部阶段属同一交付：真实Claude仅追加6测试，缺API编译失败单列declaration RED；真实Claude实现注册/映射/retained book但不接advance，host在真实Disk/Acquire运行上核验Pending-vs-Active behavioral RED；真实Claude接实际推进/动作丢弃/发布reset消费/phase guard后，6GREEN、完整Rust/release、实际非空GoCompanion、strictOpenSpec及精确SHA独立review。不可将声明/双重或手工seed激活当作实际producer验收。

单写者7路径/精确签名/错误优先级/6个具体fixture/oracle及英文注释要求详见英文计划。新增源文件仅由主 Agent创建空白路径metadata供冻结Edit-only使用，所有Rust正文由实际Claude写。继续使用已核实b9 frozen，不改主Loom、路由、秘密、安全、安装或output上限，nullcallsrounds及有限deadlinecleanup保留。

完成后沿原依赖进入acquisition；存储非空bootstrap/save/cache、其他dirty发布/config/gameplay/fullGo/audit/整体4.1/4.2仍开放。不推送、合并或部署。Architecture skill: no change.

2026-10-05 验收oracle修正（任务仍开放）：新激活伙伴速度为0，首个Move[1,0]yaw0按ground acceleration40×dt0.05加速到2，再移动2×dt0.05；X应8.600000381469727/0x4109999a，而不是Snow已预设vx4.3夹具的8.715。Go step.go106/motion.go9、Native step.rs235–248/321/359及collision.rs278–281和独立f32核对一致。只改计划与实际Claude测试期待，生产motion不改；首次激活、动作隔离和完整inventory/look断言保留。

2026-10-05 局部验收runtime 60f89185273e2df58e259818e843f956653d259a，仅3.7l3cr：真实本地Claude四任务六调用全部reaped，其中测试首次/注册首次permission_denied路径误读保留，实际范围内Edits已独立核验，其他调用completed。声明RED不算行为；准确fa0ecf的1控制PASS/5生产者RED转6真实Ready/Native GREEN。E0609与根错误4.3首步oracle修正史保留，首步40×0.05=2/X0x4109999a，不改生产motion。qualifiedGREEN为701628脏测试产物，hash等于60f8；准确60f8全Rust75套件3422零失败忽略/replay513/persistence293，fmt/Clippy、release、非空Go42顶层/含子用例69、OpenSpec128，GPT6.1-sol medium SCOPED_PASS。原3861行字节不变，既有扫描/Native/companion规则及通用Pending夹具未改。bootstrap/save/cache/自动acquisition/assembledruntime/其余源结果及整体4.1/4.2仍开放；文档SHA与runtime分开核对，未推送合并部署。Architecture skill: no change.
