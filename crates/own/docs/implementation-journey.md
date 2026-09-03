# 把 Austral 线性检查接到 JavaScript 上：实现历程和 tradeoff

ownershipjs 用 oxc 解析 JS/TS，读 `/*#own` 注释，按 Austral 的线性表检查 unique / affine / copy / `&readonly` / `&mut`。没有运行时，不改写源码。这篇文章记的是把「表算法已经对了」推进到「unique 不能悄悄消失」时实际踩过的坑，以及每次选择背后的取舍。

## 1. 真正的洞不在表上，在没走到的 AST

线性表（Unconsumed / BorrowedRead / BorrowedWrite / Consumed）在「注解过的函数、裸 identifier」这条路径上本来就是对的。整仓正确性审查里漏掉的，几乎全是 **visitation**：某个 unique 生产者（`Buffer.from("x")`、unique 参数、`/*#own let`）出现在检查器没走进去的节点上，于是既没有 unique-forget，也没有 use-after-move。

典型形状：

- 程序顶层没有 `push_scope`，顶层 unique 绑定静默消失。
- `await` / 括号 / `as` / `void` / 逗号把 unique 返回值包一层，`call_return_type` 只认裸 Call。
- 未注解的外层函数直接跳过 body，里面的 `/*#own let` 和嵌套注解函数都不查。
- `finally` 被忽略；try 里 consume、finally 再 consume 不是 use-after-move。
- 对象方法、类字段、计算键、spread、解构默认值、rest、装饰器、JSX、`export =`、namespace/enum……每补一块，review 就在相邻节点上再找出一块。

对策不是再抽象一层 visitor，而是 **按 oxc 节点补 walk**，每条洞配一条走 `check_source` 的测试。检查器现在有两套重叠的走法：

- `check_expr` / `count`：identifier 的消耗、借用、exclusive `&&`/`?:`。
- `check_discard`：unique **rvalue** 被丢掉（语句、void、copy/readonly 实参、模式默认值）。

两者必须同时覆盖。只走 `count` 会漏掉匿名 `Buffer.from("x")`；只走 `check_discard` 会漏掉 `buf` 被用两次。

## 2. 几个反复出现的 tradeoff

### peel 值和类型注解

`peel` 剥括号、`await`、TS `as` / `!` / `satisfies` / instantiation，让 `Buffer.from` 的 unique 返回能穿过包装。这是对的——对 **值**。

代价：`check_discard` 如果 `match peel(expr)`，`as { [Buffer.from("x")]: number }` 这种 **类型里的 unique 生产者** 永远走不到 `TSAsExpression` 臂。后来改成：值身份继续 peel；类型注解在 peel 之前单独 `walk_type_wrappers`。

`ident_name` 也踩过同样的坑。给 `const x = await buf` 做 move 时让 `ident_name` 穿过 `&&`，结果 `consume((flag && buf) as any)` 被当成「对 ident `buf` 的 consume」，再加上 exclusive 走 right，变成 false use-after-move。所以 ident 查询拆成两个：

- `ident_name`：peel + sequence 最后一项 + assignment RHS。给 `count_arg` 用。仍然 **不** 跟 `&&` / `||` / `?:`，否则 `true && buf` 会被当成裸 ident，exclusive 计数失效。
- `ident_move_src`：再加 assignment、`?:` 两臂同一 ident、`&&` 右操作数、`||`/`??` 在左不是 call 时才看右。只给 `const` 绑定和 `instance_sig` 的 fallback 用。

`peel` 始终不剥 `AssignmentExpression`。`check_discard` 有独立的 assignment-target 臂；赋值只在具名 helper 里跟 `.right`（`call_return_type`、`as_call`、`ident_name`、`collect_object_*_methods_at`）。全局 peel 赋值会让 `obj[Buffer.from("x")] = y` 丢掉计算键上的 unique。

`instance_sig` 用 `ident_name.or_else(ident_move_src)`，这样 `(true && buf).filter()` 能查到 `Buffer#filter`，而 `consume(true && buf)` 仍走 exclusive。

### `check_contained_fn` 和 `enter_body`

嵌套函数必须进 body，否则 `(function () { Buffer.from("x"); });` 是静默泄漏。但 var 初始化里的 `/*#own type: () => unique Buffer */ const f = () => Buffer.from("x")` 必须先用 **声明偏移** 查签名，否则会按未注解箭头把返回值当成 discard。

`enter_body: HashSet<span>`：每个函数/类/对象 body 只检查一次，第一次带上的 annotation offset 生效。var 声明里 **先** `check_fn_or_arrow_init`（完整 offset），再 `check_expr`，后者撞上 HashSet 就跳过。类字段则 **先** `check_methodish_expr` 再 `check_expr`，避免 `check_contained_fn` 只用 `[fn.span.start]` 把带属性注释的箭头锁死成未注解。

代价：HashSet 是「第一次 wins」。走错顺序就会用错签名。顺序是不变量，不是优化。

### 未知 callee consume vs copy unique-forget

未知名字的调用（`dec(Buffer.from("x"))`、用户函数没写 `/*#own`）按 **consume** 处理 unique 实参：所有权交给 callee，不 unique-forget。copy / `&readonly` 实参位置则 unique-forget。这和「匿名 unique 传给 `console.log` 必须报」一致，也解释了为什么 `@dec(Buffer.from("x"))` 不是泄漏——那就是一次未知调用。

`new Foo(...)` 一度只按 consume 扫参数，绑定 `const x = new Foo(Buffer.from("x"))` 时 copy 构造参数会把 unique 吃掉。后来 `new` 和 Call 一样查 `FnSig`。

### fluent `copy this` vs `copy`/`clone`/`filter`

Prelude 生成器把「`this: &...` 且返回同一类型」收成 `copy T`，这样 `a.on(...)`、`buf.slice(0)` 当语句不会 unique-forget。这是 Node 流式 API 的实用选择。

必须排除的：

- 方法名 `copy` / `clone` / `duplicate`：`Hash#copy` 是新的 unique Hash，不是 view。
- `Buffer#filter` / `map`：新的 `Uint8Array`，不是 `slice`/`subarray`。
- `Type.prototype.method` 没有 `this:` 参数，一度整表留在 `unique T`，`Agent.prototype.on(...)` 假 unique-forget。rewrite 时用 `Type#method` 的 this 类型做同样的 demotion。

这些规则写在 `scripts/gen-prelude.cjs` 的 `rewriteRetKind`，对已生成的 `.own` 跑 `rewrite-existing`（本轮没有 Corsa/tsgo 全量盘点）。

两遍 lookup 只对 **至少三段的点名** 做（`http.globalAgent.on`）：把 `Buffer.from` 这种静态工厂按 `Buffer#from` 的 fluent `this` 降成 copy，会让 `Buffer.from("x")` 不再 unique，整仓 checker 测试塌掉。`stream.compose` 同样必须留 unique。

### flatten origin 和解构

对象模式从数组取值时，`{ 1: o } = [0, ...spread]` 的 origin 不是 0。`collect_object_*_methods_at` 必须带着 origin 穿过 assignment / sequence / logical / yield；掉进 origin-0 的 helper 会把 `[elem]` 当成 `{0: elem}`，`o.make()` 查不到方法。for-of 的对象模式必须先剥到 `ArrayExpression` 再走元素循环，不能把数组当数字键对象。

### unique `this` 和绑定 copy 返回

`Buffer.from("x").toString()` 当语句：toString 是 copy，receiver 必须 unique-forget。

`const s = Buffer.from("x").toString()`：曾经 `call_return_type` 是 copy 就 `continue`，整棵 init 不再 `check_discard`，receiver 消失。现在 copy 返回落到 `check_discard`；unique 返回才 skip 对 **被绑定那个 call** 的 unique-forget，但仍 `check_call_subexprs` 扫 callee / 实参。

反过来，`fs.promises.open("x").close()`：`FileHandle#close` 的 `this` 是 unique，receiver 是转移不是 discard。`check_call_callee` 看 `instance_sig` 的 this 模式，Consume 且 receiver 不是 ident 时走 `check_call_subexprs` 而不是 `check_discard`。

`instance_sig` 起初只认 ident receiver。`getHash().copy()` 找不到 `Hash#copy`。现在非 ident receiver 用 `call_return_type(object).type_name()` 拼 `Type#method`。

### tagged template 的第 0 个参数

`` uniqueTag`hello` `` 要当 `uniqueTag(...)` 查返回类型。插值对应 **跳过 TemplateStringsArray 的 param 1..n**，和实例方法跳过 `this` 一样。用 index `i` 会把 `${Buffer.from("x")}` 对到 strings 的 copy 参数上，假 unique-forget。

### try / finally 快照

`return` 在 try-with-finally 里不能当场 `require` unique，因为 finally 还要跑。做法是压一份 table 快照，finally 前恢复。

坑：

- 单层 `Option` 会被内层 finally 偷走，改成 `Vec`。
- 快照里 **不要** 把 unique 标成 Consumed，否则 catch 再 return 会看到已消耗。
- catch 的 consume **不能** 写进 return 快照：catch 不是 return 路径。
- 检查器在 `if (flag) return` 之后仍继续走 `consume(buf)`。第一份快照是 return 当时的表；live 表会被后面的 consume 改掉。finally 若只跑快照，fallthrough 上的「try consume + finally consume」是静默 double consume。现在快照和 fallthrough 表不一致时，finally **跑两遍**（先 exit 路径，再 fallthrough），用第二遍抓 use-after-move。

这仍然不是完整的 CFG。throw+catch+finally 的路径集合比这更大。

### exclusive `&&` / `||` / `?:`

`count` 对 logical/conditional 返回空，交给 `check_expr` 的分支表 + `visit_exclusive_maybe`。漏掉 template、spread、计算键、yield 时，`` `${flag && buf}`; consume(buf) `` 是静默 double-use。补的是 `visit_exclusive_nested` 的父节点，不是把 `count` 改回去（否则又会两边都算 consume）。

`||` / `??` 一度总是 discard 右操作数。`consume(null ?? Buffer.from("x"))` 假 unique-forget。现在：左是 unique 生产者则转移左、discard 右；否则转移右。

## 3. `/review` 循环本身是一种检查器

计划要求 uncommitted diff 上 `/review` 到 **0 Severity: bug**。实际发生的是：每修一类静默泄漏，下一轮 review 就在 **相邻 AST 或相邻语义** 上再打一条 CLI 复现。这不是同一条 bug 修不掉，而是「unique 不能悄悄消失」在 JS+TS 上几乎没有闭合的节点集合。

选择是：每条 CLI 复现都补测试再补 walk，不在检查器里写测试源码的特殊情况；prelude 政策留在生成器里，不手改几千行 `.own`。测试始终打 `check_source` / `check_source_with` / `check_paths`——和 CLI 同一条路径。

`file.sigs` 的收集必须和 `check_*` 走进去的节点对齐。只 walk body 不 collect 签名时，`/*#own type: () => unique Buffer */ function inner()` 的 body 会被查，但丢掉的 `inner();` 查不到 prelude/file 表，仍然静默。namespace、static block、对象字面量、IIFE 都要 collect。

还没当成闭合 CFG 的部分：throw 是否被当前 try 的 catch 接住、generator 的 `yield` 跨 resume、完整的 TS 类型查询（Corsa/tsgo 在本轮不可用）。那些是明确的后续，不是「表算法再拧一拧」。`/review` 对这份检查器会不断在相邻 AST 上找洞；闭合策略是每条 CLI 复现进测试，而不是假装节点集合有限。

## 4. 验收时看什么

- `cargo test`：库单测 + `tests/checker.rs`（走 shipped API）。
- CLI `ownershipjs --check` 对同一批 fixture 跑两次，输出必须一致。
- `examples/` 的 ok/err 文件仍然对得上。
- Node vs `--runtime none`：`buf.toString()` / `console.log(buf)` 在 Node 下不 consume；`FileHandle#close` 仍然 move；`none` 把未知名字当 consume。

注释只写不变量（例如 body 只进一次是为了保住 annotation offset），不写「我们改了什么」。生成器启发式的 why 留在 `gen-prelude.cjs` 和这篇里。
