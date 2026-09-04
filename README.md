# pragmajs

JavaScript / TypeScript 的**注释驱动静态检查**。`/*#… */` 是 pragma，不是 JSDoc。

两个检查器各是一个 **library crate**。`/*#own` 和 `/*#rt` 的分析没有合并；parse、semantic graph、Corsa 类型查询是共用入口。用户入口是统一 CLI：parse 一遍，然后 own 和 rt 一起跑。

| crate | 注释 | 做什么 |
| --- | --- | --- |
| [`pragmajs`](crates/pragmajs) | — | 统一 CLI。每个文件 parse 一次，再跑 own 和 rt。 |
| [`pragma-own`](crates/own) | `/*#own … */` | Austral 线性 / 借用。无运行时。原 **ownershipjs**。库，无独立二进制。 |
| [`pragma-rt`](crates/rt) | `/*#rt … */` | Flux 风格 refinement types + Z3。可选 `__rt.assert`。原 **refinejs**。库，无独立二进制。 |
| [`pragma-parse`](crates/parse) | — | 共用 oxc parse + semantic graph。 |
| [`pragma-corsa`](crates/corsa) | — | 共用 Corsa 类型查询。仅 rt；own/wasm 不链。 |
| [`pragma-loc`](crates/loc) | — | UTF-8 offset → 行/列。own 诊断和 rt 注解共用。 |

```bash
cargo test -p pragma-own
cargo test -p pragma-rt
cargo test -p pragmajs

cargo run -p pragmajs -- check crates/own/examples/
cargo run -p pragmajs -- check crates/rt/fixtures/sqrt.js
cargo run -p pragmajs -- build --target ecmascript crates/rt/fixtures/sqrt.js output.js
```

消融实验、gold corpus、基线反例与复现命令见
[`docs/ablation.md`](docs/ablation.md)。

仓库：[github.com/AkaraChen/pragmajs](https://github.com/AkaraChen/pragmajs)。独立仓库 [refinejs](https://github.com/AkaraChen/refinejs) 已停更，指向这里。

## License

MIT
