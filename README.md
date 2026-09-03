# pragmajs

JavaScript / TypeScript 的**注释驱动静态检查**。`/*#… */` 是 pragma，不是 JSDoc。

两个检查器各是一个 crate，共享的只有仓库、CI 和「用 oxc 读注释」这件事。解析、prelude、求解器和运行时**没有**抽成公共库——refinejs 那套基建证明了硬抽一层并不划算。

| crate | 注释 | 做什么 |
| --- | --- | --- |
| [`pragma-own`](crates/own) | `/*#own … */` | Austral 线性 / 借用。无运行时。原 **ownershipjs**。 |
| [`pragma-rt`](crates/rt) | `/*#rt … */` | Flux 风格 refinement types + Z3。可选 `__rt.assert`。原 **refinejs**。 |

```bash
cargo test -p pragma-own
cargo run -p pragma-own -- --check crates/own/examples/

cargo test -p pragma-rt
cargo run -p pragma-rt -- check crates/rt/fixtures/sqrt.js
```

仓库：[github.com/AkaraChen/pragmajs](https://github.com/AkaraChen/pragmajs)。独立仓库 [refinejs](https://github.com/AkaraChen/refinejs) 已停更，指向这里。

## License

MIT
