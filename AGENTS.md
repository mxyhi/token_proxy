# AGENTS.md - Token Proxy 代码库指南

本文件为在 `token_proxy` 项目中工作的 AI 智能体提供指导。

## 项目概览

**Token Proxy** 是基于 Tauri 的 AI API 代理工具，用于转发 OpenAI、Gemini、Anthropic 等 AI API 格式，支持本地运行、token 使用统计、负载均衡和优先级管理。

- 前端: React 19 + TypeScript + Vite + Tailwind CSS v4 + shadcn/ui(pnpm dlx shadcn@latest add xxx)
- 后端: Rust (Edition 2021) + Tokio + Axum
- 桌面框架: Tauri 2
- 代理转发/转换参考: [QuantumNous/new-api](https://github.com/QuantumNous/new-api)

---

## 命令参考

### 开发命令

```bash
npm run dev        # 启动开发服务器（Vite + Tauri，端口 1420，HMR 端口 1421）
npm run build      # 构建生产版本（TypeScript 编译 + Vite 构建）
npm run preview    # 预览生产构建
npm run tauri      # Tauri 相关命令
```

### Rust 测试(自动进行测试)

```bash
cd src-tauri && cargo test                # 运行所有测试
cd src-tauri && cargo test --bin main     # 运行二进制测试
cd src-tauri && cargo test <test_name>    # 运行单个测试
cd src-tauri && cargo test -- --nocapture # 查看测试输出
```

### TypeScript 编译检查(自动进行测试)

```bash
npx tsc --noEmit    # 类型检查但不生成文件
```

---

## 代码风格指南

### 导入顺序

1. React hooks 和核心函数
2. 第三方库（Tauri, Radix UI 等）
3. 本地模块（使用 `@` 别名）
4. CSS 导入（放在最后）

```typescript
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import "./App.css";
```

### 命名约定

- **变量/函数**: camelCase（语义化描述性强）
- **组件/类型**: PascalCase
- **常量**: UPPER_SNAKE_CASE
- **Hook**: `use` 前缀

```typescript
const EMPTY_FORM: ConfigForm = {
  /* ... */
};
function validate(form: ConfigForm) {
  /* ... */
}
function useConfigState() {
  /* ... */
}
interface HeaderSectionProps {
  /* ... */
}
```

### TypeScript 规则

- **严格模式**: `tsconfig.json` 启用了 `strict: true`, `noUnusedLocals`, `noUnusedParameters`
- **避免 any**: 永远不使用 `as any`、`@ts-ignore`、`@ts-expect-error`
- **错误类型**: 使用 `unknown` 捕获错误，类型守卫转换
- **类型定义优先**: 在使用前定义类型
- **不显式声明返回值类型**: TypeScript 可推断时省略

```typescript
// ✅ 正确
function parseError(error: unknown) {
  if (error instanceof Error) return error.message;
  return String(error);
}

// ❌ 错误
function parseError(error: any) {
  /* ... */
}
```

### 错误处理

- 使用 `unknown` 而非 `any`
- 异步操作使用 try-catch + 状态管理
- 验证函数返回对象而非抛出错误（便于 UI 显示）

```typescript
async function loadConfig() {
  try {
    const response = await invoke<ConfigResponse>("read_proxy_config");
    setConfigPath(response.path);
  } catch (error) {
    setStatus("error");
    setStatusMessage(parseError(error));
  }
}

function validate(form: ConfigForm): ValidationResult {
  if (!form.host.trim()) {
    return { valid: false, message: "Host is required." };
  }
  return { valid: true, message: "" };
}
```

### 注释风格

- **极少注释**: 代码自解释为主
- **配置文件**: 可以使用注释说明选项
- **复杂逻辑**: 仅在无法通过命名表达意图时添加注释

```typescript
// ✅ 自解释代码，无需注释
function toForm(config: ProxyConfigFile): ConfigForm {
  return {
    host: config.host,
    port: String(config.port),
    localApiKey: config.local_api_key ?? "",
  };
}
```

### 状态管理模式

三层分离架构（参考 `src/App.tsx`）:

1. **原始状态**: `useConfigState()` - useState 管理
2. **派生状态**: `useConfigDerived()` - useMemo 计算
3. **操作**: `useConfigActions()` - useCallback 封装

```typescript
function useConfigState() {
  const [form, setForm] = useState<ConfigForm>(EMPTY_FORM);
  return { form, setForm /* ... */ };
}

function useConfigDerived(form: ConfigForm) {
  const validation = useMemo(() => validate(form), [form]);
  return { validation };
}

function useConfigActions({ setForm }) {
  const saveConfig = useCallback(async () => {
    /* ... */
  }, [setForm]);
  return { saveConfig };
}
```

### 组件模式

- **Props 类型**: 显式定义 Props 接口
- **data-slot 属性**: 每个组件添加 `data-slot` 属性
- **cn 函数**: 所有 className 通过 `cn` 函数合并
- **组件变体**: 使用 `class-variance-authority` (cva)

```typescript
import { cn } from "@/lib/utils";

type ButtonProps = {
  variant?: "default" | "destructive" | "outline";
  className?: string;
};

export function Button({
  variant = "default",
  className,
  ...props
}: ButtonProps) {
  return (
    <button
      data-slot="button"
      data-variant={variant}
      className={cn(buttonVariants({ variant, className }))}
      {...props}
    />
  );
}
```

---

## 文件结构

### 前端目录（TypeScript + React）

```
src/
├── main.tsx              # React 入口（ReactDOM.createRoot）
├── App.tsx               # 主应用组件（563 行，接近拆分上限）
├── components/ui/        # shadcn/ui 组件
├── lib/utils.ts          # 工具函数
└── assets/               # 静态资源
```

### 后端目录（Rust）

```
src-tauri/src/
├── main.rs               # Rust 入口点
├── lib.rs                # Tauri 命令和应用逻辑
├── proxy.rs              # 代理服务模块
└── proxy/                # 代理子模块
    ├── config.rs
    ├── usage.rs
    └── log.rs
```

---

## 技术配置

### TypeScript 配置（tsconfig.json）

- **目标**: ES2020
- **严格模式**: 启用
- **模块系统**: ESNext + bundler resolution
- **路径别名**: `@/*` → `./src/*`
- **React Compiler**: 启用（通过 Babel 插件）

### Vite 配置

- **开发端口**: 1420
- **HMR 端口**: 1421
- **严格端口**: 启用（端口占用时失败）
- **忽略目录**: `src-tauri/**`

### Tailwind CSS

- **版本**: v4.1.18
- **插件**: `@tailwindcss/vite`
- **动画**: `tw-animate-css`

### Rust 依赖（Cargo.toml）

- **异步运行时**: Tokio 1.49.0（多线程）
- **Web 框架**: Axum 0.7.9
- **HTTP 客户端**: Reqwest 0.12.26（JSON + 流式）
- **序列化**: Serde + serde_json（JSONC 配置文件）
- **桌面框架**: Tauri 2

---

## 代码质量规则

### 文件大小限制

- **单文件**: 超过 588 行必须模块化拆分
- **单个函数**: 超过 58 行必须拆分

### 性能原则

- **异步 I/O**: 优先使用异步操作
- **单一职责**: 函数仅做一件事
- **代码复用**: 抽离重复逻辑，逻辑只有一个实现

### 代码简洁性

- **删除冗余**: 删除无用代码、变量声明、函数
- **避免防御性编程**: KISS 原则，不过度抽象
- **类型体操**: 你是一个类型体操选手，善用 TypeScript 类型系统

### 类型安全

- **禁止 any**: 所有代码必须有明确类型
- **类型推断**: TypeScript 可推断时省略显式类型声明

---

## Tauri 命令

前端通过 `invoke()` 调用的 Rust 命令：

- `read_proxy_config` - 读取代理配置
- `write_proxy_config` - 写入代理配置

新增命令时：

1. 在 `src-tauri/src/lib.rs` 定义 Tauri 命令
2. 在前端使用 `invoke<TCommandName>()` 调用
3. 定义 TypeScript 类型用于响应

---

## 调试技巧

### 前端调试

- 使用浏览器开发者工具（React DevTools）
- 查看网络请求（Tauri 命令调用）
- 控制台日志（避免生产代码残留）

### 后端调试

- 使用 `println!` 或 `eprintln!` 输出日志
- 查看 `data.db`（SQLite 请求统计）
- 使用 Rust 调试器（如 VS Code 的 rust-analyzer）

---

## 常见陷阱

1. **端口冲突**: Vite 固定端口 1420，确保端口可用
2. **类型转换**: 表单输入是 string，需转换为 number（如端口）
3. **空值处理**: 使用 `??` 运算符处理 `null`/`undefined`
4. **Rust 异步**: Tokio 运行时必须启用 `rt-multi-thread` 特性
5. **React Compiler**: 不需要手动 memoization（useMemo/useCallback 可省略）
