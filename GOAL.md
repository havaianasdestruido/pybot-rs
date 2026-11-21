# GOAL — Porte 100% do PyBot para Rust (somente `std`)

## Objetivo

Reescrever **toda** a engine/codebase do PyBot (fork de `huggingface/pybot`) em Rust, **sem dependências externas** (nem crates, nem Python em runtime), com:

1. **Desempenho absoluto** (latência por turno, tempo de startup, memória).
2. **Estabilidade** acima de tudo: nada de panic em produção, nada de tempo exponencial, resultado determinístico.
3. **Equivalência comportamental** com o original Python, medida por teste diferencial.

> Prioridade quando houver conflito: **estabilidade > equivalência > desempenho**. Desempenho só vale se os dois primeiros estiverem verdes.

## Contexto (a confirmar na Fase 0)

Números abaixo vêm de descrições públicas do projeto, não de leitura do código. Valide tudo na Fase 0.

- ~640 mil linhas em ~528 arquivos Python; sem LLM: regex, condicionais e tabelas de consulta.
- Camadas: `bot/modules/` (handlers + arbiter), `bot/understanding/` (parsing, typos, coreferência), `bot/memory/` (estado de diálogo), `bot/knowledge/` (fatos como dados, a maior parte do código), `bot/compose/` (frases e fallbacks), `pybot.py` (CLI: `/reset`, `/exit`, `-p "pergunta"`).
- Existe uma suíte de testes grande, escrita por agentes — será o nosso **oráculo**.

## Regras do projeto

- **Somente `std`.** `Cargo.toml` sem `[dependencies]`. Ferramentas de desenvolvimento (scripts Python de extração de dados, comparação) são permitidas, mas **nunca** em runtime.
- `#![forbid(unsafe_code)]` em todo o crate. Única exceção: módulo `ffi` da Fase 11 (WASM), isolado e revisado.
- Sem `unwrap()`/`expect()`/indexação que possa dar panic fora de testes. Lints: `clippy::unwrap_used`, `clippy::indexing_slicing`, `clippy::panic` em `deny`.
- Determinismo: nunca iterar `HashMap` para decidir saída (a ordem é aleatória no Rust). Usar `Vec`, `BTreeMap` ou mapa próprio com ordem de inserção (Python preserva ordem de `dict`).
- Cada fase só fecha quando seu **critério de saída** passa no CI.

## Estrutura alvo

```
pybot-rs/
  Cargo.toml              # sem dependências
  build.rs                # opcional: gera tabelas estáticas (só std)
  data/                   # dados exportados do Python (binário/TSV), versionados
  src/
    lib.rs  main.rs       # main = CLI equivalente ao pybot.py
    util/                 # strings estilo Python, ordered map, RNG, edit distance
    regex/                # engine própria (sem backtracking)
    unicode/              # normalização/casefold mínimos necessários
    knowledge/
    understanding/
    memory/
    compose/
    modules/              # handlers + arbiter
    ffi.rs                # só Fase 11
  tests/                  # port dos testes + diferencial + fuzz
  tools/                  # scripts de dev (Python permitido aqui)
  bench/
```

---

## Fase 0 — Reconhecimento e baseline

Entender o que existe antes de escrever uma linha de Rust.

- [x] Clonar `huggingface/pybot`, fixar o commit de referência.
- [x] Inventário: linhas por pasta, arquivos de dados, tamanho do `knowledge/`.
- [x] Inventário de **imports**: stdlib usada (`re`, `random`, `json`, `difflib`, `unicodedata`, `collections`, `itertools`, `datetime`…) e qualquer dependência de terceiros.
- [x] Inventário de **features de regex** realmente usadas: lookahead/lookbehind, backreferences, quantificadores lazy/possessivos, grupos nomeados, flags (`I`, `M`, `S`, `X`, `U`), classes Unicode (`\w`, `\b` com Unicode).
- [x] Caça a **dinamismo Python**: `eval`, `exec`, `getattr`, `__import__`, registro de módulos por decorator/reflexão, monkey patching.
- [x] Fontes de **não-determinismo**: `random`, `time`, `set` iterado, `hash()`, `id()`.
- [x] Profiling do original (`cProfile`, `py-spy`): onde o tempo vai (regex? carga de dados? dispatch?).
- [x] **Corpus dourado**: gerar milhares de entradas (dos testes + sessões multi-turno + fuzz de texto) e gravar para cada turno: resposta, módulo vencedor, estado de memória relevante.

**Saída:** relatório `docs/recon.md` com os inventários, o perfil de desempenho e o corpus dourado versionado. A lista de features de regex define o escopo da Fase 2.

## Fase 1 — Fundação e harness

- [ ] Criar o crate, lints, CI (`cargo build`, `clippy`, `test`, formatação), MSRV fixa.
- [ ] Definir tipos centrais: `Turn`, `Session`, `Reply`, `ModuleId`, `Error` (sem `Box<dyn Error>` solto).
- [ ] **Runner diferencial**: executa o corpus dourado no Rust e compara com o Python, relatório por diretório/módulo (% idêntico, diffs agrupados).
- [ ] Esqueleto do `main.rs`: `-p "pergunta"`, REPL com `/reset` e `/exit` (std: `std::io::stdin().lines()`).

**Saída:** CI verde com o runner funcionando (0% de equivalência é aceitável; o harness é o entregável).

## Fase 2 — Substitutos de stdlib e crates (`util/`, `regex/`, `unicode/`)

A fase mais arriscada de “só `std`”. Tudo o que o Python dava de graça precisa existir.

**2.1 Engine de regex própria**
- [ ] Parser de sintaxe compatível com o subconjunto de `re` usado (Fase 0).
- [ ] Execução **sem backtracking**: NFA estilo Thompson / Pike VM, com tempo O(n·m) garantido → elimina ReDoS por construção.
- [ ] Captura de grupos, grupos nomeados, flags, semântica de `search`/`match`/`fullmatch`/`finditer`/`sub`/`split`.
- [ ] Features fora do modelo regular (backreferences, lookaround): decidir **caso a caso** com base no inventário — reescrever a regra à mão, ou implementar suporte limitado com limite de passos.
- [ ] Teste diferencial da engine contra `re` do Python em um corpus de (padrão, texto) extraído do próprio PyBot + fuzz.
- [ ] Preparar o terreno para a Fase 10: API que permita compilar **vários padrões num autômato só** (tipo `RegexSet`).

**2.2 Utilitários estilo Python**
- [ ] Funções de string com semântica Python: `split`/`strip`/`casefold`/`title`/`isalnum`/etc., `str.format` e f-strings usados (formatação de float inclusive).
- [ ] Mapa com ordem de inserção; ordenação estável (`sort_by` do Rust já é estável).
- [ ] Edit distance / similaridade (substituto de `difflib` se usado), com resultados idênticos ao Python nos casos usados.

**2.3 Unicode**
- [ ] O `std` não normaliza (NFD/NFKD). Mapear o que o PyBot realmente usa (provavelmente remover acentos latinos, casefold) e gerar tabelas mínimas a partir de `unicodedata` via script de dev.

**2.4 Aleatoriedade**
- [ ] O `std` não tem RNG utilizável. Se o bot usa `random`, implementar **MT19937 + `random.seed`/`choice`/`shuffle`/`random()` idênticos ao CPython**, para que a mesma seed gere a mesma saída nos testes. Em produção, a seed vem de `SystemTime`/`RandomState`.

**Saída:** engine de regex com 100% de acordo com `re` no corpus de padrões; utilitários com testes unitários portados; RNG idêntico ao CPython para as chamadas usadas.

## Fase 3 — Dados e `knowledge/`

O maior volume de código é conhecimento como dados — o melhor candidato a virar tabela estática.

- [ ] Script de dev (Python) que **exporta** `knowledge/` para formato compacto e versionado em `data/` (binário próprio ou TSV), com checksum.
- [ ] Carregamento: `include_bytes!`/`include_str!` + parsing zero-cópia (fatias `&'static str`/`&'static [u8]`); sem I/O em runtime.
- [ ] Índices: tabelas ordenadas + busca binária, ou hash perfeito gerado em `build.rs`; strings internadas (IDs `u32`).
- [ ] Equivalência: toda consulta do corpus dourado retorna os mesmos fatos que o Python.

**Saída:** API `knowledge::*` equivalente, startup medido, nenhum acesso a disco em runtime.

## Fase 4 — `understanding/` (parsing, typos, coreferência)

- [ ] Tokenização e normalização idênticas ao original.
- [ ] Correção de typos (provavelmente edit distance + dicionário): mesma ordem de candidatos e desempates.
- [ ] Coreferência/resolução de entidades contra o estado de memória.
- [ ] Portar os testes desta camada e passar o diferencial por subpasta.

**Saída:** saída do estágio “understanding” idêntica ao Python em 100% do corpus dourado (comparar a estrutura intermediária, não só a resposta final).

## Fase 5 — `memory/` (estado de diálogo)

- [ ] Modelar o estado com **structs e enums tipados** (nada de dict aninhado).
- [ ] `/reset`, histórico de turnos, slots e entidades; serialização só se o original persiste algo.
- [ ] Limites explícitos (tamanho máximo de histórico/sessão) para estabilidade de memória em sessões longas.

**Saída:** sessões multi-turno do corpus dourado terminam com o mesmo estado observável.

## Fase 6 — `compose/` (frases e fallbacks)

- [ ] Templates, concordância, variações e fallbacks; usar o RNG da Fase 2 onde o original sorteia.
- [ ] Mesma lógica de escolha em empates.

**Saída:** com entrada fixa e seed fixa, a composição é idêntica byte a byte.

## Fase 7 — `modules/` e arbiter

- [ ] Substituir registro dinâmico por **registro estático** (lista/array de módulos em ordem fixa; trait simples ou `fn` pointers).
- [ ] Portar os handlers por grupo, mantendo a pontuação e o critério de desempate do arbiter.
- [ ] Expor o “módulo vencedor” por turno (o PyBot mostra qual módulo respondeu) — vira o principal sinal de depuração do diferencial.

**Saída:** módulo vencedor **e** resposta idênticos ao Python em 100% do corpus dourado.

## Fase 8 — CLI, I/O e paridade total

- [ ] `main.rs` completo: banner, `/reset`, `/exit`, `-p`, códigos de saída, tratamento de EOF/Ctrl-D.
- [ ] Codificação: UTF-8 estrito na entrada, com tratamento definido para bytes inválidos (sem panic).
- [ ] Rodar **todos os testes portados** + o corpus dourado completo.

**Saída (marco “paridade”):** ≥ 99,9% de equivalência no corpus; 100% das diferenças restantes documentadas e aprovadas (ex.: bugs do original que decidimos não replicar).

## Fase 9 — Robustez: fuzz, limites, soak

- [ ] Fuzzer próprio (std): geração e mutação de entradas (Unicode estranho, textos enormes, strings vazias, bytes inválidos, sequências adversariais para regex).
- [ ] **Orçamento por turno:** limite de tamanho da entrada, de passos da engine e de memória; estourou → resposta de fallback, nunca panic ou travamento.
- [ ] `std::panic::catch_unwind` na fronteira de cada turno como última rede de proteção (e teste que prova que nenhum panic é alcançável).
- [ ] Soak test: milhões de turnos, sessões longas, checagem de vazamento (memória estável).

**Saída:** zero panics, zero timeouts e memória plana em N horas de fuzz/soak.

## Fase 10 — Desempenho

Só começa depois do marco de paridade.

- [ ] Suite de benchmarks própria (`std::time::Instant` + `std::hint::black_box`): latência p50/p99 por turno, startup, memória de pico, throughput.
- [ ] Baseline contra o Python no mesmo corpus.
- [ ] **Regex em lote:** unir os padrões de cada estágio num único autômato (ou DFA preguiçoso com cache limitado) em vez de um laço de matches.
- [ ] **Dados:** índices compactos, interning, layout contíguo (`Vec<u32>` em vez de árvores de ponteiros), evitar alocação por turno (reutilizar buffers, arenas simples baseadas em `Vec`).
- [ ] **Arbiter:** curto-circuito por prioridade; pontuação em paralelo com `std::thread::scope` apenas se o benchmark provar ganho.
- [ ] Perfil: `perf`/`samply` externos (ferramentas de dev), sem nada novo no crate.
- [ ] Cada otimização exige: benchmark antes/depois **e** diferencial ainda em 100%.

**Saída:** metas numéricas definidas na Fase 0 (ex.: p99 por turno, startup) atingidas, sem regressão de estabilidade.

## Fase 11 — WASM (opcional, só `std`)

- [ ] Alvo `wasm32-unknown-unknown` (ou `wasm32-wasip1`) sem `wasm-bindgen`: funções `extern "C"` exportadas + cola JS mínima escrita à mão.
- [ ] `unsafe` restrito ao módulo `ffi.rs`, com revisão e testes; resto do crate continua `forbid(unsafe_code)`.
- [ ] Substitui a demo com Pyodide; comparar tamanho do `.wasm` e latência.

**Saída:** demo no navegador com paridade de respostas e o painel “módulo vencedor”.

## Fase 12 — Release e congelamento

- [ ] Versão 1.0, `CHANGELOG`, documentação de arquitetura e das divergências aceitas em relação ao Python.
- [ ] CI com: diferencial completo, fuzz curto, benchmarks com limiar de regressão.
- [ ] Remover o código Python do caminho de build/execução (mantê-lo apenas em `tools/` e como referência).

---

## Definição de pronto

- [ ] `Cargo.toml` sem dependências; `cargo build --release` em máquina limpa.
- [ ] Nenhum `unsafe` fora de `ffi.rs`; nenhum `unwrap`/panic alcançável.
- [ ] ≥ 99,9% de equivalência com o PyBot original no corpus dourado; divergências documentadas.
- [ ] Todos os testes originais portados e passando.
- [ ] Fuzz/soak sem falhas; uso de memória estável.
- [ ] Metas de desempenho atingidas e protegidas por benchmark no CI.

## Riscos principais

| Risco | Mitigação |
|---|---|
| Regex do Python com features não regulares (backrefs, lookaround) | Inventário na Fase 0; reescrever regras ou suporte limitado com orçamento de passos |
| Sem crates: JSON/Unicode/RNG/regex precisam ser escritos | Fase 2 dedicada; escopo limitado ao que o PyBot usa de fato |
| Dinamismo Python difícil de traduzir | Mapear na Fase 0; trocar por registro estático |
| Divergências sutis (ordem de dict/set, float, Unicode) | Diferencial contínuo desde a Fase 1; comparar estágios intermediários, não só a saída |
| Volume enorme de código (centenas de milhares de linhas) | Portar por camada, na ordem de dependência (dados → entendimento → memória → composição → módulos), com critério de saída por fase |
| Otimização prematura quebrar equivalência | Fase 10 só após paridade; toda otimização passa pelo diferencial |
