# @ctx-traits/config

Typed TypeScript surface for `ctx` config documents. `config.ts` never
evaluates at load time — `ctx traits config build` compiles it into
`config.toml` once, ahead of any command that reads config.

## Install

```sh
pnpm add -D @ctx-traits/config
```

## Source

https://github.com/ctx-company/traits (`packages/config`).
