# Fase 1 — Fundação e harness

Relatório da Fase 1 do [`GOAL.md`](../GOAL.md). Tudo aqui foi medido neste repositório, com a
toolchain fixa (`rustc 1.97.0`), e pode ser reproduzido pelos comandos indicados.

> **Saída da fase:** CI verde com o runner funcionando (0 % de equivalência é aceitável; o
> harness é o entregável). — `GOAL.md`, Fase 1.

O critério é atendido: o crate compila, `clippy` está limpo com `-D warnings`, `cargo fmt`
não muda nada, os testes passam, e o runner diferencial reexecuta as 3 062 turnos do corpus
dourado no bot em Rust e publica um relatório por bloco e por módulo. A equivalência começa
em 2,78 % (respostas) e 0,56 % (módulo vencedor) — o número honesto de uma fundação com
módulos de esqueleto, e o ponto de partida que as fases seguintes têm de subir.

## Os quatro itens da fase

### 1. Crate, lints, CI e MSRV

| Peça | Onde | O que garante |
|---|---|---|
| MSRV fixa | `rust-toolchain.toml` (`channel = "1.97.0"`), `Cargo.toml` (`rust-version = "1.97.0"`) | a mesma `rustc` para todo mundo, inclusive CI |
| Sem dependências | `Cargo.toml` sem `[dependencies]`, `Cargo.lock` com um único pacote | a regra "somente `std`" não depende de disciplina |
| Sem `unsafe` | `#![forbid(unsafe_code)]` em `src/lib.rs` + `unsafe_code = "forbid"` nos lints | provado pelo compilador |
| Sem `panic` em produção | `unwrap_used`, `expect_used`, `indexing_slicing`, `panic`, `todo`, `unimplemented` em `deny` | nenhum caminho de entrada derruba o processo |
| Determinismo | `HashMap`/`HashSet` proibidos por teste; `Vec`/`BTreeMap`/`Map` ordenado por inserção | mesmo corpus, mesmo relatório, em qualquer máquina |
| CI | `.github/workflows/ci.yml` | `fmt --check`, `build` (debug e release), `clippy --all-targets -D warnings`, `test` (debug e release, 3 SOs), guarda "std-only" e o passo do diferencial |

`cargo clippy --all-targets -- -D warnings` termina sem uma única advertência: os 74 avisos
iniciais viraram correções de código (e não `#[allow]`s), com três exceções justificadas e
documentadas — as variáveis de trabalho `a`–`e` do SHA-1, os cinco booleanos que espelham o
`argparse` em `Options`, e as cinco flags independentes de `Faults`.

### 2. Tipos centrais

| Tipo | Arquivo | Contrato |
|---|---|---|
| `Turn` | `src/types.rs` | texto exato, sem normalização; `Turn::from_bytes` é *lossy* e nunca falha (como `sys.stdin` do Python) |
| `Session` | `src/session.rs` | memória + log de turnos limitado; `reset` esvazia os dois |
| `Reply` | `src/types.rs` | texto do bot + `is_blank` usando o whitespace do Python |
| `ModuleId` | `src/types.rs` | índice `u16` no `MODULES`; nunca uma `String` comparada por igualdade em ordem de saída |
| `Error` | `src/error.rs` | `#[non_exhaustive]` com `Io`, `PathEncoding`, `InvalidUtf8`, `Json`, `Corpus`, `Manifest`, `Registry`, `Limit` — **sem** `Box<dyn Error>` solto |

O `Error` implementa `std::error::Error` com `source()` só para `Io`; o resto carrega
contexto em campos tipados (`Corpus { line, message }`), o que permite testar a mensagem de
cada variante — há um teste que percorre todas.

### 3. Runner diferencial (o entregável)

```
cargo run --release --bin differential -- tests/golden
cargo run --release --bin differential -- tests/golden --json r.json --text r.txt --quiet
cargo run --release --bin differential -- tests/golden --min-module-percent 50   # portão de CI
```

Cinco sinais medidos por turno: **resposta** normalizada, **módulo vencedor**, **lista de
candidatos** (módulos e prioridades, em ordem), **chaves de memória** e **digest SHA-1 da
memória completa**. Sessões são reexecutadas inteiras e em ordem, porque a memória de um turno
depende dos anteriores; o digest é o sinal mais duro e o único que não perdoa nada.

Saídas: texto legível (por bloco, por módulo esperado, por módulo candidato, com até três
exemplos agrupados por grupo) e JSON de chaves ordenadas, para consumo por ferramentas.
Códigos de saída: `0` relatório produzido, `1` erro de escrita, `2` corpus inválido, `3`
portão (`--min-*`) não atingido.

### 4. `main.rs`

```
pybot                       chat interativo (stdin é terminal)
pybot -p "pergunta"         uma resposta e sai
pybot -i -p "oi"            começa com -p e continua o chat
echo "linha" | pybot        uma resposta por linha
```

`/reset` esvazia a conversa (limpando a tela num TTY, como o `pybot.py`), `/help` mostra a
ajuda, `/exit`, `/quit` e `/bye` saem, Ctrl-D sai. O laço lê com
`BufRead::read_until(b'\n')` sobre `stdin().lock()` — o equivalente direto do `input()` do
Python com `\r` removido — e escreve por um `Streams` injetável, o que permite testar o CLI
inteiro em memória, sem processo filho.

## Medições (reproduzíveis)

Corpus: `tests/golden/corpus.jsonl`, 3 062 turnos em 2 850 sessões,
SHA-256 `d119d44c12a3313fac546320b764e122cf983f19b4897e30beb5afc7bbfb2fba`
(conferido pelo teste `the_corpus_file_matches_the_manifest_byte_for_byte`).

| bloco | turnos | resposta | módulo | candidatos | chaves de memória |
|---|---:|---:|---:|---:|---:|
| curated | 512 | 15,04 % | 0,20 % | 0,20 % | 7,23 % |
| fuzz | 704 | 0,43 % | 1,14 % | 1,14 % | 1,99 % |
| knowledge_probes | 1 846 | 0,27 % | 0,43 % | 0,43 % | 1,73 % |
| **total** | **3 062** | **2,78 %** | **0,56 %** | **0,56 %** | **2,71 %** |

Digest de memória: **0,00 %** — esperado e documentado. O `understood` do Python é um
dicionário de ~60 chaves produzido pelo motor de entendimento (Fase 4); o esqueleto grava as
chaves de topo que o pipeline precisa (`said_kind`, `user`, `intent_wants`,
`frequency_answer`, `frequency_reply`, `turns`), mas não o conteúdo, então o SHA-1 não fecha.
O relatório mostra zero em vez de esconder o bloco.

Falhas de handler durante os 3 062 turnos: **0**.

Leitura do relatório por módulo esperado: `memory_store` acerta 60,38 % das respostas e
`chat` 16,31 % — as duas famílias cujo texto o esqueleto reproduz literalmente. `answers`
(1 571 turnos, a base de conhecimento da Fase 6) fica em 0,00 %, e é a maior lacuna isolada.

## Divergências conhecidas e aceitas

Documentadas para que ninguém as "conserte" por engano:

1. **Sem a linha "Loading PyBot…"** — o Python leva segundos para importar; o Rust carrega
   instantaneamente e uma linha de status custaria mais do que anuncia.
2. **UTF-8 inválido no stdin** é lido como perda (`U+FFFD`) em vez de derrubar a leitura: o
   `Turn::from_bytes` é total, como o `sys.stdin` do Python.
3. **Pipe quebrado** (`pybot | head`) termina com sucesso; o Python levanta `BrokenPipeError`.
4. **`-s/--seed`** é aceito e registrado, mas ainda não tem efeito: o RNG do Python
   (`random.seed`) chega na Fase 2.4.

## Testes

| suíte | quantidade | o que cobre |
|---|---:|---|
| `cargo test --lib` | 220 | unidade: tipos, memória, JSON, hash (SHA-1 e SHA-256 contra vetores FIPS), pyutil, arbiter, engine, runtime, módulos, CLI, corpus |
| `tests/golden.rs` | 29 | ponta a ponta: integridade do corpus e do manifesto, blocos, replay inteiro, determinismo, relatório texto e JSON, o binário real via `CARGO_BIN_EXE_differential` |
| `tests/registry.rs` | ver arquivo | a tabela de 109 módulos exportada do Python, hooks, prioridades, unicidade |
| `tests/style_guards.rs` | 15 | as regras do projeto como testes: sem dependências, sem `unsafe`, sem `panic` fora de testes, sem relógio/RNG, sem `HashMap`, MSRV = toolchain, CI roda todos os portões |
| `src/bin/differential.rs` (testes) | 8 | parsing de opções, portões, corpus ausente, `--help` |

Os testes de estilo leem os fontes e falham o build se a regra for quebrada — a regra deixa de
ser um parágrafo no `GOAL.md` e passa a ser um teste.

## Ferramentas de desenvolvimento

- `tools/recon/gen_corpus.py` — gera o corpus dourado a partir do bot Python
  (`CORPUS_SEED = 20261121`, `PYTHONHASHSEED=0`).
- `tools/export_registry.py` — exporta a tabela `MODULES` do Python para
  `src/modules/registry.rs` (109 linhas, ordem `(-priority, name)`).
- `tests/test_cli.py` — a suíte do CLI original, mantida como referência (não roda no CI Rust).

## Como verificar

```bash
cargo build --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo run --release --bin differential -- tests/golden
```

O último comando imprime a tabela acima. Nenhuma etapa precisa de rede.

## Próxima fase

Fase 2 (`util/`, `regex/`, `unicode/`) — os substitutos de biblioteca padrão que as fases
seguintes usam: normalização de texto, distância de edição, RNG compatível com o `random` do
Python, e a engine de regex própria. O runner diferencial, já pronto, mede cada passo.
