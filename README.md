# ownershipjs

面向 JavaScript / TypeScript 的**静态所有权 / 借用检查器**。

用 [oxc](https://oxc.rs) 解析 `.js` / `.ts`，读取 `/*#own ... */` 注释，报告 use-after-move、double-move、可变借用冲突和生命周期错误。

**没有运行时**：不生成、不注入、不改写源码。算法来自 Austral 的线性检查（[How Austral’s Linear Type Checker Works](https://borretti.me/article/how-australs-linear-type-checker-works)）。语法、状态表和可靠性边界见 [`docs/design.md`](docs/design.md)。

## 安装

```bash
cargo install --path .
```

或直接在仓库里跑：

```bash
cargo run -- --check examples/
cargo test
```

## 用法

```bash
ownershipjs --check path/to/file.js
ownershipjs --check examples/
ownershipjs --check --runtime none examples/ok-prelude-console.js
ownershipjs --check --runtime bun file.js
ownershipjs --check --runtime deno file.js
```

`--runtime` / `-r` 选择内置函数 prelude：`node`（默认）、`bun`、`deno`、`none`（不带）。签名来自 Corsa/tsgo 对 `@types/node` / `bun-types` / Deno dts 的盘点（`scripts/gen-prelude.cjs`），包括实例方法（`Buffer#toString`、`FileHandle#close`）。`buf.toString()` 按绑定上的类型名查找。文件里的 `/*#own type:` 会覆盖 prelude。

无诊断时退出码 `0`，有所有权 / 借用错误时退出码 `1`：

```
examples/err-unique-forget.js:11:18: error[unique-forget]: unique value `buf` is not consumed
```

只检查带了 `/*#own type: ... */` 的函数。未注解的函数当普通 JS。

## 注解

### 类型

| 写法 | 规则 |
| --- | --- |
| `unique T` | **恰好消费一次**（线性）。忘记消费或 move 后再用都是错误。 |
| `affine T` | **最多消费一次**。作用域结束时静默 drop 可以。 |
| `copy T` | 任意复用。 |
| `&readonly T` | 不可变借用。 |
| `&mut T` | 可变借用。同一表达式里不能重叠 `&mut`，也不能和 `&readonly` 混用。 |
| `void` | 没有被拥有的返回值。 |

把 `unique` / `affine` 传给同种参数是 **move**。`void buf` 也是消费。`buf.field` 是 path 读取，不消费。

被调函数参数写成 `&readonly T` / `&mut T` 时，这次调用只借用实参，不 move。

### 函数签名

```js
/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own
 * type: (buf: unique Buffer) => void
 */
function process(buf) {
  consume(buf);
}

process(make());
```

`process(make())` 里如果漏了 `consume(buf)`，会报 `unique-forget`。

### 局部、词法借用、clone、drop

```js
/*#own let x: unique Buffer */
const x = make();

/*#own borrow buf as view: &readonly Buffer */
const view = buf;          // 借用，不是 move；活到当前块结束

/*#own borrow! buf as mutv: &mut Buffer */
const mutv = buf;

/*#own clone buf as copy */
const copy = buf;          // 复制一份；`buf` 仍未消费

/*#own drop buf */         // 显式消费

read(/*#own &readonly */ buf);
write(/*#own &mut */ buf);
```

借用不能 return / 赋出当前块（`borrow-escape`）。被借用期间不能消费 owner。

## 诊断

| slug | 含义 |
| --- | --- |
| `unique-forget` | `unique` 值没有被消费 |
| `use-after-move` | move 之后又用 |
| `double-move` | 同一次表达式里消费两次 |
| `consume-in-loop` | 循环外定义、循环内消费 |
| `branch-inconsistent` | 分支结束后状态不一致 |
| `borrow-after-move` | move 之后再借 |
| `consume-while-borrowed` | 借用期间消费 |
| `mut-borrow-conflict` | 重叠 `&mut`，或 `&mut` 和 `&readonly` 冲突 |
| `borrow-escape` | 借用逃出词法块 |
| `unmapped` | 未建模的 JS 构造（见下方限制） |
| `annot-parse` | `/*#own` 注释写坏了 |

## 例子

`examples/ok-*` 应检查通过。`examples/err-*` 应报出文件名对应的规则。

| 文件 | 在演示什么 |
| --- | --- |
| `ok-unique-move.js` / `err-unique-forget.js` / `err-unique-use-after-move.js` / `err-unique-double-move.js` | unique move |
| `ok-affine-drop.js` / `err-affine-use-after-move.js` | affine 可 drop vs use-after-move |
| `ok-readonly-borrow.js` / `err-consume-while-borrowed.js` | `&readonly` |
| `ok-mut-borrow.js` / `err-overlapping-mut.js` / `err-readonly-mut-conflict.js` | `&mut` |
| `ok-lifetime-scope.js` / `err-borrow-escape.js` | 词法 region |
| `ok-copy.js` / `ok-copy.ts` / `ok-clone.js` | Copy / Clone |
| `ok-branch-consume.js` / `err-branch-inconsistent.js` / `err-consume-in-loop.js` | 分支一致、循环深度 |
| `ok-prelude-console.js` / `err-prelude-buffer-forget.js` | Node prelude：`console.log` 不消费；`Buffer.from` 返回 unique |
| `ok-prelude-buffer-tostring.js` / `ok-prelude-handle-close.js` | 实例方法：`Buffer#toString` 不消费；`FileHandle#close` 消费 this |

```bash
cargo run -- --check examples/ok-unique-move.js    # exit 0
cargo run -- --check examples/err-unique-forget.js # error[unique-forget]
```

## Playground

浏览器里跑同一个检查器（WASM，无服务器）：

```bash
./scripts/build-playground.sh
# 打开 playground/index.html，或：
cd playground && vercel --prod
```

## 限制

单文件。跨文件调用按**函数名**匹配，且被调方必须在**同一个文件里**带了注解。

这些 JS 构造**没有建模**（报 `unmapped`，不会假装没问题）：`eval`、`with`、对 owned 值的计算属性、prototype / `__proto__` 改写、捕获 owned 绑定的嵌套函数。

算法与可靠性假设见 [`docs/design.md`](docs/design.md)。

## License

MIT
