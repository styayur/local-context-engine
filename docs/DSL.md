# The Local Search DSL

A small, deterministic query language for the local machine. It exists so that
power users, scripts and AI agents can express a search without going through
natural language, and so that whatever compiles a query has one obvious target.

## Grammar

```text
query  := token*
token  := key ":" value | word
key    := type | ext | path | name | drive | modified | created | size
        | state | pid | user | visible | sort | limit
value  := quoted | bare
```

Parsing **never fails**. A token whose key is not recognised stays in the
free-text part of the query, so `localsearch "C:\Users\me"` is a search rather
than a syntax error.

## Keys

| key        | values                                                              | meaning                                     |
|------------|---------------------------------------------------------------------|---------------------------------------------|
| `type`     | `file`, `directory`/`folder`, `process`, `app`, `service`, `window` | restrict the entity types; comma-separated  |
| `ext`      | `rs`, `.rs`, `pdf,toml`                                             | extension, without or with the leading dot  |
| `path`     | any substring, quote when it has spaces                             | full-path substring                         |
| `name`     | any substring                                                       | entity-name substring                       |
| `drive`    | a letter                                                            | restrict to one volume                      |
| `modified` | `<24h`, `>7d`, `>=2024-01-01`, `<=2024-01-01`                       | last-write time                             |
| `created`  | same operators as `modified`                                        | creation time                               |
| `size`     | `>10mb`, `<=512kb`, `=1gb`, `10mb`                                  | file size (`10mb` means "at least")         |
| `state`    | `running`, `stopped`, `paused`, `auto`, `manual`, `disabled`        | service state or start type                 |
| `pid`      | an integer                                                          | process id                                  |
| `user`     | any substring                                                       | owning account (needs `account_lookup`)     |
| `visible`  | `true` / `false`                                                    | window visibility                           |
| `sort`     | `relevance`, `name`, `path`, `modified`, `created`, `size`, `pid`, `memory` × `-asc`/`-desc` | explicit order   |
| `limit`    | `1`–`2000`                                                          | maximum results                             |

### Time operators

| spelling           | meaning                            |
|--------------------|------------------------------------|
| `modified:<24h`    | newer than 24 hours ago            |
| `modified:>7d`     | older than 7 days ago              |
| `modified:>=2024-01-01` | on or after an absolute date  |
| `modified:<=2024-01-01` | on or before an absolute date |

Durations accept `s`, `m`, `h`, `d` and `w`. Sizes accept `b`, `kb`, `mb`, `gb`
and `tb`.

## Examples

```text
chrome
type:file rust
type:file ext:rs
type:file ext:pdf modified:<24h
type:process python
type:process name:node
type:app vscode
type:service state:running
type:window github
path:projects ext:toml
drive:C ext:exe sort:size-desc
path:"My Projects" ext:toml
size:>10mb sort:size-desc limit:20
```

## Natural language

The same inputs also accept natural language. The compiler is a deterministic
rule set, not a model, and both languages compile to **the same** `SearchQuery`:

| English                        | Simplified Chinese        | compiled query                                  |
|--------------------------------|---------------------------|-------------------------------------------------|
| `recent PDFs`                  | `最近的 PDF`              | `type:file ext:pdf sort:modified-desc`          |
| `running python processes`     | `正在运行的 python`       | `type:process python`                            |
| `find VS Code`                 | `找 VS Code`              | `VS Code` (all types)                            |
| `yesterday modified rust files`| `昨天修改的 rust 文件`    | `type:file ext:rs modified:<24h`                 |
| `past 30 days rust files`      | `过去30天修改的 rust 文件`| `type:file ext:rs modified:<30d`                 |

`localsearch --explain` and the MCP `compile_query` tool both show the compiled
DSL, the stage that produced it and the rules that fired.

## Precedence

1. If the input contains a recognised `key:value` token, the **DSL** wins and
   the remaining words become free text.
2. Otherwise the **natural language** rules run.
3. Otherwise the input is a **plain keyword** query.

Text matching itself requires every query token to match, against the entity
name first and then against its path. That is why
`type:file ext:rs modified:<24h` matches nothing when no Rust file changed today
rather than quietly dropping the predicate.