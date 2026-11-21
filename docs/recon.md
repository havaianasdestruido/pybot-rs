# Fase 0 — Reconhecimento e baseline

Documento gerado em 2026-11-21. Toda a análise é estática (AST) ou de execução do bot Python
original, **sem** modificar `bot/`. Os scripts que produziram estes números estão em
[`tools/recon/`](../tools/recon/) e podem ser re-executados:

```sh
python3 tools/recon/inventory.py        # inventários estáticos -> inventory.json, regex_inventory.json
PYTHONHASHSEED=0 python3 tools/recon/gen_corpus.py --out tests/golden/corpus.jsonl --manifest tests/golden/manifest.json
PYTHONHASHSEED=0 python3 tools/recon/verify_corpus.py tests/golden/corpus.jsonl
PYTHONHASHSEED=0 python3 tools/recon/profile_bot.py
```

Ambiente: Python 3.11.2 (o PyBot declara suporte a 3.8+), Linux x86-64, 2 CPUs, 4 GB RAM,
relógio UTC. `PYTHONHASHSEED=0` foi usado em toda execução cujo resultado é comparado byte a byte.

---

## 1. Commit de referência

O workspace (`havaianasdestruido/pybot-rs`) **é** uma cópia byte a byte do PyBot upstream, mais o
`GOAL.md`:

| Repositório | Commit | Relação |
|---|---|---|
| `huggingface/pybot` `main` | `7d63c7fbebb44c7e466e5c8b5c1f341ab767929e` (2026-11-16) | idêntico ao workspace em todo o conteúdo, exceto `GOAL.md` (verificado com `diff -r`) |
| `huggingface/pybot` branch `bot-24cbc04` | `5b1658136585dd05a8b54bf3dcc4055269a892bb` (2026-11-14) | `main` difere desta branch apenas em `README.md` e `assets/` |
| workspace (`arena/be6ce140-pybot-rs`, base `main` = `e74cadf1feeca86491484915b012c0034478f75d`, 2026-11-21) | `e74cadf` | conteúdo == upstream `main` + `GOAL.md` |

`pybot.py` declara `SOURCE_COMMIT = 24cbc0439e07fd015c0747ab3be6e782a42b5c5b` — o commit do
repositório do enxame de agentes de onde `bot/` foi copiado. Esse commit **não é alcançável**
público: `git fetch origin 24cbc04...` responde `upload-pack: not our ref` no repositório
`huggingface/pybot`, e não existe repositório público `HuggingFaceSwarms/pybot`. A branch
`bot-24cbc04` é o espelho mais próximo desse snapshot e é idêntica a `main` em `bot/`.

**Fixado para o projeto:** `bot/` nesta branch == `huggingface/pybot@7d63c7f` (== branch
`bot-24cbc04@5b16581` em `bot/`). Qualquer regressão futura de conteúdo será detectada pelo
corpus dourado (seção 8), não por diff textual. O commit do swarm (`24cbc04…`) fica registrado
aqui apenas para rastreabilidade; não é obtível.

A árvore `bot/` não contém nenhum arquivo que não seja `.py` — o que parece "dado" é literal
Python (ver seção 2). O bot não lê arquivo algum em runtime: `grep -rn "open(" bot` retorna zero
chamadas (a única ocorrência de `open` é a de leitura de stdin do `pybot.py`/`bot/__main__.py`).

---

## 2. Inventário: linhas por pasta, arquivos, `knowledge/`

Total de linhas Python no repositório: **≈ 638 mil** (físicas, `wc -l` sobre todos os `.py`;
~636 600 em `bot/`), **534 arquivos `.py`** no total (528 em `bot/`, 1 `pybot.py`, 1
`tests/test_cli.py`, 4 de `tools/` desta Fase 0). O README do projeto aponta "~640 mil linhas,
~528 arquivos" — confere.

| Caminho | Arquivos | Linhas | % linhas |
|---|---:|---:|---:|
| `bot/knowledge/` | 159 | 316 852 | 49,7 % |
| `bot/compose/` | 126 | 104 286 | 16,4 % |
| `bot/memory/` | 117 | 109 319 | 17,2 % |
| `bot/modules/` | 110 | 80 926 | 12,7 % |
| `bot/understanding/` | 13 | 25 026 | 3,9 % |
| `bot/` (raiz: `runtime.py`, `__init__.py`, `__main__.py`) | 3 | 140 | <0,1 % |
| `pybot.py` | 1 | 198 | <0,1 % |
| `tests/test_cli.py` | 1 | 162 | <0,1 % |
| `tools/recon/` (Fase 0) | 4 | 1 127 | 0,2 % |
| **total** | **534** | **638 034** | 100 % |

Maiores arquivos (todos dados embutidos como literais):

| Linhas | Arquivo |
|---:|---|
| ~1,0 M | `bot/knowledge/family.py`, `bot/modules/chat.py`, `bot/knowledge/home_craft.py`, `bot/compose/fallback.py`, `bot/knowledge/cooking_entries.py` (arquivos de ~0,9–1,1 MB) |

**Arquivos de dados:** não existem como arquivos. `find bot -type f ! -name '*.py'` (excluindo
`__pycache__`) retorna zero. O `knowledge/` são 159 arquivos `.py`, **22 MB**, importando
literais Python (`ENTRIES`, listas, dicts, frozensets) avaliadas em tempo de importação.
Implicação direta: tudo vira tabela estática em `data/` (Fase 3) com `include_bytes!`;
nenhum I/O em runtime; o parser de literais Python não precisa existir no crate Rust.

Outros números relevantes:

- Módulos com `respond` definido: **97** (de 110 arquivos em `bot/modules/`); com `observe`: **16**;
  com `after`: **2**; com `PRIORITY` literal de módulo: **103**.
- Domínios de `bot/knowledge/` declarando `ANSWERS` (especialização de tópico para o arbiter):
  **158** arquivos no total entre `knowledge/` e `modules/`.
- O enxame não deixou suíte de testes no repositório: `tests/` só tem `test_cli.py` (12 testes
  do CLI, passam 12/12 em ~30 s). A suíte grande "escrita pelos agentes" **não está neste
  repositório** — o oráculo executável disponível é o corpus dourado (seção 8).
- Não-ASCII: 179 arquivos com caracteres fora de U+0000–U+007F, ~4 839 caracteres no total —
  predominantemente moeda (£, €), pontuação tipográfica (—, ’) e acentos latinos/skandinavos.
  Não há uso de CJK/Árabe/Hangul.

---

## 3. Inventário de imports

Análise AST de todos os `.py` (`tools/recon/inventory.py`). Zero dependências de terceiros:
o único "terceiro" encontrado é `pybot` (o próprio CLI, importado por `tests/test_cli.py`).

| Módulo stdlib | Imports | Módulo stdlib | Imports |
|---|---:|---|---:|
| `re` | **419** | `pkgutil` | 2 |
| `datetime` | **57** | `ast` | 2 |
| `random` | **27** | `unicodedata` | 2 |
| `string` | 13 | `hashlib` | 2 |
| `copy` | **13** | `json` | 2 |
| `sys` | 9 | `unittest` | 2 |
| `math` | 5 | `pathlib` | 2 |
| `importlib` | **4** | `decimal` | 2 |
| `os` | 4 | `fractions` | 1 |
| `calendar` | 3 | `difflib` | **1** |
| `argparse` | 3 | `itertools` | 1 |
| `operator` | 1 | `time` | 1 |
| `readline` | 1 | `functools` | 1 |
| `contextlib` | 1 | `io` | 1 |
| `collections` | 1 | `multiprocessing`, `subprocess` | 2 |

- Imports relativos (`from . import ...`) na intra-`bot`: **1 569** ocorrências; imports
  absolutos de `bot.`: 53.
- `importlib` é usado **dinamicamente** em 4 arquivos, sempre pelo mesmo padrão
  `importlib.import_module("." + str(name), __package__)` seguido de `getattr(module, NAME)`:
  `bot/runtime.py` (carregamento dos módulos de `bot/modules/`), `bot/knowledge/__init__.py`
  (carregamento preguiçoso de domínios), `bot/memory/cause.py` (evita import circular com
  `..modules.<name>`) e `bot/modules/arbiter.py` (~15 ocorrências sondando hooks `DECLARED_*`
  dos módulos pelo nome). Não há import dinâmico "aberto": todos os nomes vêm de parâmetros
  internos ou de `pkgutil.iter_modules` sobre diretório fixo.
- `pkgutil.iter_modules` em 2 lugares (`runtime.py`, `knowledge/__init__.py`).
- `copy.deepcopy` (13 imports, 21 chamadas): o de maior impacto é `runtime._copy`, que faz
  `deepcopy` da memória **uma vez por módulo candidato** (~97) a cada turno, descartando quase
  todas as cópias; só a do vencedor é retida. Idempotência barata ou snapshot estrutural é um
  dos maiores ganhos disponíveis na Fase 5/10.
- `difflib`: **1** uso — `difflib.SequenceMatcher(None, a, b).ratio() >= 0.7` em
  `bot/modules/answers.py:495`. Portar o algoritmo Ratcliff–Obershelp ou substituir por
  equivalente documentado (decisão na Fase 2).
- `decimal`/`fractions`: `bot/knowledge/maths.py` usa `Fraction` (aritmética exata para
  "3/4", "1 1/2" em receitas) com `limit_denominator(64)`, e `ROUND_HALF_UP` de `Decimal`.
- `datetime` (57 imports): **zero** chamadas a `now()`/`today()`/`utcnow()`/`fromtimestamp`
  (ver seção 6). Uso é de datas fixas e aritmética de calendário (`calendar`, 3 imports).
- `unicodedata`: apenas 2 usos, ambos NFKD + remoção de marcas combinantes
  (`understanding/normalize.py` e `knowledge/study.py`). `casefold()` **zero**; `.lower()` com
  6 047 ocorrências. `.isalpha()/.isdigit()/.isalnum()/.isspace()/.isupper()/.islower()` têm
  ~320 ocorrências e, em Python, são Unicode-aware — o Rust precisa de tabelas equivalentes
  (ver seção 4 e riscos).
- `hashlib`: 2 usos (digests internos, fora do caminho quente).
- `json`: 2 usos (CLI/diagnóstico, não no caminho do turno).
- `ast`: 2 usos (CLI/testes de lint interno).

---

## 4. Features de regex realmente usadas

12 594 padrões resolvidos estaticamente em 419 arquivos (de **13 676** call-sites que
recebem uma regex; **1 082** não puderam ser lidos — padrões montados em runtime com `%`,
concatenação com partes dinâmicas ou bytes) contendo **11 318 padrões únicos** (o enxame
gerou muita regex quase idêntica). 10 197 `re.compile`, 1 653 `search`, 651 `findall`,
499 `sub`, 480 `escape`, 322 `split`, 174 `match`, 101 `finditer`, 77 `fullmatch`, 2 `subn`.

### 4.1 Flags usadas

| Flag | Call-sites |
|---|---:|
| `re.I` / `IGNORECASE` | 5 751 + 328 |
| `re.I \| re.S` / `I \| X` / `X` / `VERBOSE` / `S` / `IGNORECASE\|VERBOSE` | 10 + 9 + 22 + 5 + 4 + 2 |
| `re.X` inline | 7 usos de `(?i:...)` no próprio padrão (inline) |
| flags numéricas (`1`, `0`) | 20 |

### 4.2 Features estruturais (12 594 call-sites resolvidos lêem padrões completos; totais sobre os resolvíveis)

| Feature | Ocorrências | Decisão para a Fase 2 |
|---|---:|---|
| `\b` / `\B` (fronteira de palavra) | **9 812** | essencial; \b Unicode com tabelas (ou aproximação documentada) |
| classes `\w \d \s` e negadas | **7 087** | essencial; Unicode-aware |
| quantificadores lazy (`*? +? ?? {m,n}?`) | 725 | tratado naturalmente por uma Pike VM (semântica esquerda) |
| lookahead `(?=...)`/`(?!...)` | **683** | implementar na VM com orçamento de passos |
| grupos nomeados `(?P<name>...)` | **589** | trivial na VM |
| lookbehind `(?<=...)`/`(?<!...)` | **412** | implementar na VM (todos de largura fixa, como `re` exige) |
| âncoras `\A`/`\Z` | 21 | trivial |
| inline `(?i)` / `(?i:...)` | 7 | trivial |
| backreferences numeradas `\1`–`\99` | **7 sites** | riscar: reescrever à mão (padrões `(.)\1+`, `\b(\w+)(\s+\1\b)+`) |
| backreference nomeada `(?P=name)` | **1 site** | riscar: reescrever à mão (`bot/memory/ack.py:6679`) |
| atomic groups/possessivos `(?>`, `*+` | 0 | não existe em `re`; os 2 "hits" eram falsos positivos (`\?+`, `[ ?!]+`) |
| padrões inválidos para `re` | **0** | os "INVALID_IN_PY_RE" do scanner eram artefatos do avaliador estático (constantes reatribuídas `BODY = BODY[:-1] + ")"` e `_FUNERAL_NOUN += ...`); após seguir `+=` e `NAME[:-1]`, **todos os 12 594 padrões resolvidos compilam com o `re` do Python** |

**Toda regex resolvida compila com o `re` do Python.** Os lookbehinds são todos de largura fixa
por construção (`re` rejeita largura variável na compilação, e o bot carrega — qualquer padrão
inválido derrubaria o import ou ficaria atrás de try/except silencioso — o scanner, após
corrigir o avaliador estático, confirma: **zero** padrões inválidos).

**Conclusão para o escopo da Fase 2:** a engine própria precisa de `\b` e classes Unicode
(latinas pelo menos), grupos nomeados/numerados, flags `I`, `S`, `X` (e `M`/`U` por
completude), quantificadores lazy e **lookaround limitado com orçamento de passos**.
Backreferences não entram na engine: 8 sites são reescritos manualmente ou executados como
casos especiais com limite. Nenhum uso de `(?#`, possessivos ou condicionais.

Padrões únicos com contagens, features por padrão e primeiro local de ocorrência:
[`tools/recon/regex_inventory.json`](../tools/recon/regex_inventory.json) (4,2 MB, versionado
para servir de corpus de teste diferencial da Fase 2).

### 4.3 `re.escape` e montagem dinâmica

`re.escape` usado 480 vezes — muitos padrões são `re.escape(trecho)` encadeados em literais,
o que explica parte dos 1 082 não resolvidos; esses padrões são quase todos literais com
fronteiras `\b`, i.e., casos simples. O inventário dos não resolvidos é recriado por execução:
o corpus dourado exercita o caminho real com `PYTHONHASHSEED=0`.

---

## 5. Caça a dinamismo Python

| Padrão | Usos | Sites / notas |
|---|---:|---|
| `eval` / `exec` | **0** | — |
| `__import__` direto | **0** | — |
| `execfile` | **0** | — |
| `delattr` | **0** | — |
| `compile()` (builtin) | 0 no bot | 1 falso positivo (o scanner conta `re.compile`) |
| `getattr` | **209** | 189 com valor literal e default `None` (padrão "hook opcional": `getattr(module, "TAKES_AS_IT_COMES", None)`); 20 não-literais, todos do par `import_module("."+str(name))` + `getattr` no arbiter/runtime/cause |
| `setattr` | 1 | só em `tests/test_cli.py` |
| `globals()` / `vars()` | 12 | `understanding/lexicon.py:1988` e `knowledge/entertainment.py:5397` (agregam tabelas do módulo em vocabulários lazy); `memory/repeat.py:1666`; `compose/fallback.py:11503`; conteúdo determinístico, ordem de iteração é a de inserção — determinística |
| `locals()` | 0 | — |
| decoradores | `_sized_by_the_chat_before` 7 (todos em `bot/knowledge/home.py`, transformação local de funções — **não** é registro); `lru_cache` 1; `unittest.skipIf` 1 |
| monkey patching | **0** | nenhuma atribuição a `<modulo importado>.<attr>` fora de testes |
| registro dinâmico de módulos | 1 sítio | `runtime.load_modules()`: `pkgutil.iter_modules` + `import_module` + `getattr(PRIORITY)` → **a substituir por registro estático** (Fase 7) |
| sondagem de hooks por nome (arbiter) | ~15 sítios | `importlib.import_module("."+module_name)` + `getattr(..., DECLARED_CLAIMS_INSTEAD / DECLARED_AS_IT_COMES / DECLARED_ABOUT_THE_CHAT / TAKES_AS_IT_COMES, None)`; em Rust vira tabela estática de capacidades |
| `importlib.import_module` em runtime | 1+15+1 | `runtime.py`, `arbiter.py` (~15), `cause.py` (2) |
| classes dinâmicas / `type(...)` 3-arg | 0 | — |
| `__dict__` direto | 0 | — |

**Conclusão:** o dinamismo se reduz a (a) carregamento/registro de módulos e (b) sondagem de
hooks declarados, ambos de conjunto **finito e conhecido estaticamente** (110 módulos, 5 hooks).
Não há lógica dependente de `eval` nem geração de código. O port pode ser 100% estático.

---

## 6. Fontes de não-determinismo

| Fonte | Ocorrências | Análise |
|---|---:|---|
| `random` (import) | 27 | `pybot.py` faz `random.seed(-s N)`; o bot usa majoritariamente `random.Random(seed)` **locais** com seed derivada de `len(turns)` ou de um `seed` passado (22 sítios, ex.: `compose/__init__.py:355`, `memory/choice.py:378`) — determinísticos dado o estado de conversa. `random.seed/setstate/getstate` globais: 9 sítios, sempre para **isolar** uma sondagem (`compose/hedge.py:49`, `memory/cause.py:383`, `modules/confusing.py:83`): salva estado, roda, restaura — sem vazamento |
| `random.choice/randrange/sample` globais | ~10 | dependem do estado global; com `-s` fixado são reproduzíveis (pybot.py semeia) |
| `time` | 1 import | `pybot.py` usa `time.perf_counter` (medição, não lógica); zero uso no bot |
| `datetime` (57 imports) | chamadas `now/today/utcnow/fromtimestamp`: **0** | uso é de datas fixas/aritmética (ex.: "dias até sexta" com `date(2024,…)`? não: sem datas correntes — o bot não sabe que dia é hoje além do que o usuário diz) |
| `hash()` | **0** | — |
| `id()` | **54** | truque de chave por identidade: `FAMILY_SAY = {id(NUTS): …, …}` em `knowledge/cooking.py` (e `SELF_SAY`): determinístico dentro de um processo (objetos singletons), mas **não portável** — Rust usa IDs explícitos de constante (Fase 3 define `const NUTS: u32` etc.) |
| `set()` / `frozenset()` | **5 272** call-sites | a esmagadora maioria é teste de pertinência/construção; não itera |
| `for x in set(...)` | **7** | `compose/fit.py:2373,4851,5312,6789,6812`, `memory/store_replies.py:8744`, `understanding/body.py:316`. Análise sítio a sítio: 6 dos 7 retornam `True` ao primeiro elemento que qualifica (ordem-irrelevante); `store_replies.py:8744` usa `min()` sobre tuplas `(-len, -start, name)` — tiebreaker único, ordem-irrelevante; o loop de `body.py` aplica substituições `\bpalavra\b` de palavras distintas (comutam). **Nenhuma não-determinismo observável** |
| `For` sobre literal conjunto | 0 | — |
| `list(set(...))` | 0 | — |
| iteração de `dict` | — | Python 3.7+ preserva ordem de inserção — determinística entre processos com a mesma entrada e mesma versão; o Rust reproduz com ordem de inserção própria (Fase 2.2) ou `BTreeMap` onde a ordem alfabética é mais barata que a de inserção |
| `hash()` aleatório (str) | mitigado | corpus e verificações rodam com `PYTHONHASHSEED=0`; no Rust, iteração só via `Vec`/`BTreeMap`/mapa com ordem de inserção (regra do GOAL) |
| ordem dos candidatos no arbiter | determinística | `runtime.load_modules()` ordena por `(-PRIORITY, nome)` — estável |
| ordem de `entries()` | determinística | `sorted(domains().items())` + ordem de definição das ENTRIES |
| qualidade do float | — | uso de `math` (5 imports) e floats no arbiter (PRIORITY `-999.5` existe); comparações com empate exato — Rust `f64` idêntico (IEEE-754); evitar álgebra que divirja (sem `mul_add`, sem fast-math) |

**Até onde sabemos, com `PYTHONHASHSEED=0` e `-s <seed>` o PyBot é determinístico**, e o corpus
dourado confirma empiricamente (verificação na seção 8).

---

## 7. Profiling do original

Medição com `tools/recon/profile_bot.py` (Python 3.11.2, `PYTHONHASHSEED=0`, 2 CPUs, dados
brutos em [`profile.json`](../tools/recon/profile.json)):

Nota: `py-spy` não foi usado — o ambiente é externally-managed (`pip install` recusado sem
`--break-system-packages`, e cProfile já identifica os hotspots com exatidão).

- **Startup**: ~3,7 s de import de `bot` em processo quente (6,9 s em processo frio, medido no
  primeiro acesso). O gargalo é compilar/importar 22 MB de literais de conhecimento.
- **Latência por turno** (mediana de 3 seeds, memória nova por prompt, **sem** profiler):

  | Métrica | Valor |
  |---|---:|
  | mean | 0,496 s |
  | p50 | 0,283 s |
  | p95 | 0,573 s |
  | p99 = max | **4,591 s** |

  O p99 é real e reproduzível: `"what's my name?"` com memória **vazia** (sem nome dito)
  leva 4,2–6,0 s em 5 execuções com seeds distintos — a linha cai por todas as malhas
  (knowledge, recall, composer) antes do `memory_store` responder "I don't know it yet".
  Com nome guardado, a mesma linha responde em < 0,2 s. Ou seja: **a malha é quase toda o
  custo**; o caminho feliz é barato.
- **RSS**: 126,7 MB após o import; 212 MB após 100 turnos (**+85 MB em 100 turnos** —
  `memory["turns"]` cresce sem limite no original; relevante para o soak da Fase 9).
- **Bateria cProfile** (12 prompts, overhead do profiler incluso; 32,1 s no total,
  76,3 M chamadas de função).

### 7.1 Por função (self time, top 12)

| ncalls | tottime | função |
|---:|---:|---|
| 158 874 | 7,74 s | `knowledge/lookup.py:5977 _the_key_names_the_thing` |
| 5 780 742 | 1,72 s | `knowledge/lookup.py:5994 <genexpr>` |
| 7 739 337 | 1,62 s | `{method 'get' of 'dict'}` |
| 3 557 190 | 1,11 s | `knowledge/lookup.py:1772 _forms` |
| 13 929 235 | 0,97 s | `{built-in method builtins.len}` |
| 6 058 733 | 0,93 s | `{method 'split' of 'str'}` |
| 211 956 | 0,83 s | `knowledge/lookup.py:2972 _part_matches` |
| 6 748 500 | 0,79 s | `{method 'lower' of 'str'}` |
| 3 126 433 | 0,78 s | `{built-in method builtins.all}` |
| 7 394 551 | 0,70 s | `{built-in method builtins.isinstance}` |
| 547 213 | 0,60 s | `{method 'search' of 're.Pattern'}` |
| 614 195 | 0,41 s | `re/__init__.py:272 _compile` (cache lookup) |

E tempo cumulativo por função-chave: `answers.py:8331 respond` ~22,3 s acumulados em
70 chamadas; `lookup.py:5128 find` ~24,7 s em 401 chamadas (o matcher varrendo entradas);
`understanding/__init__.py:1557 understand` ~4,1 s em 420 chamadas.
`copy.deepcopy`: 315 512 chamadas (~0,3 s self / 0,6 s cumulativo).

### 7.2 Onde o tempo vai (leitura)

1. **O matcher de conhecimento domina**: `lookup.py` (`_the_key_names_the_thing`,
   `_part_matches`, `_phrase_span`, `find`) responde pelos ~60–70 % do tempo de turno. É
   essencialmente **varredura de palavras/frases contra tabelas de chaves** — o alvo nº 1 da
   Fase 3 (índices compactos) e da Fase 10 (autômato único, interning).
2. **`understand`** (~4 s cumulativo / 12 %): normalização e leitura linha a linha, 6 047
   `.lower()` e centenas de `isalpha/isdigit` — barato por chamada, caro por volume.
3. **`deepcopy` de memória por módulo candidato** (~97 por turno): ~2 % sob o profiler, mas é
   alocação pura no caminho quente; na Rust, snapshots estruturais/clone sob escrita.
4. **Regex**: as compilações acontecem em tempo de import (10 197 `re.compile` no carregamento);
   no turno, 547 mil `Pattern.search` são cache-hits (~2 %). A regex em lote (Fase 10) une a
   maioria desses `search` em autômatos únicos.

---

## 8. Corpus dourado

Gerado em 2026-11-21 com `tools/recon/gen_corpus.py` (Python 3.11.2, `PYTHONHASHSEED=0`,
`corpus_seed = 20261121`; o seed por sessão é `corpus_seed*100000 + índice`, então qualquer
sessão pode ser re-executada isoladamente).

- **2 850 sessões, 3 062 turnos**, 3,9 MB JSONL (uma linha JSON por turno);
  sha256 `d119d44c12a3313fac546320b764e122cf983f19b4897e30beb5afc7bbfb2fba`
  (em [`tests/golden/manifest.json`](../tests/golden/manifest.json)).
- Composição: **512 turnos** curados (saudações, `my name is X` → `what's my name?`, dúvidas
  práticas, factuais, desabafo, chit-chat, edge cases: vazio, só pontuação, `"a"*400`,
  `"word "*200`, emoji, RTL/CJK, SQL, HTML, bytes NUL, acentos, caixa mista, typos) +
  **39 sessões scriptadas multi-turno** (~200 turnos: intro de nome → recall, pronomes
  "it/that", deriva de tópico, repetição) + **~130 turnos** de sessões-templated +
  **1 650 turnos** de fuzz (mutação seed-fixada de prompts semente: delete/dup/swap/case/insert/
  accent/digit/drop-vowel, mais 150 ruídos puros) + **900 probes de conhecimento**
  (uma pergunta por entrada da knowledge base, round-robin pelos 159 domínios, ex.:
  "tell me about radiator clatter", "how do i puncture fix" — sinais de matcher, não frases
  naturais) — os 900 probes especificamente servem para exercitar o matcher de entradas e o
  arbiter por especialidade (`ANSWERS`) longe da cauda dos prompts curados.
- O que cada turno grava: `input`, `reply` (normalizada como o `pybot.py`:
  `' '.join(str(reply).split())`), `module` (o vencedor), `candidates` (`module`+`priority`,
  na ordem do arbiter — as respostas dos candidatos não são gravadas para manter o arquivo
  pequeno), `memory_keys`, `memory_digest` (sha1 da memória **completa** canonizada),
  e a memória "relevante" (tudo menos `turns` e `understood`, valores truncados a 400
  caracteres — o `understood` é interno da Fase 4 e a Fase 4 o comparará separadamente).
- **Determinismo verificado**: `tools/recon/verify_corpus.py tests/golden/corpus.jsonl`
  re-executou 32 sessões (79 turnos, multi-turno e single-turn) num processo novo —
  **0 divergências** em reply, módulo, candidatos, memory_keys e memory_digest.
- Cobertura medida por módulo vencedor: `answers` 1 571, `greeting` 758, `chat` 325,
  `first_line_open` 260, `the_shape_of_the_answer` 57, `memory_store` 53, `safety` 33,
  `favourites` 3, `loss` 2 (**9 módulos**); 18 módulos distintos aparecem como candidatos;
  turnos com ≥ 2 candidatos: 2 307; máximo de candidatos por turno: 4; **fallback
  (`Hello!`) nunca disparou** (nem com fuzz/ruído). Resposta média 209 caracteres
  (mín 6 / máx 1 004).
- Limitações, honestamente: o corpus exercita fortemente a malha de conhecimento e saudação,
  mas **não cobre a cauda longa dos ~110 módulos** — só 9 venceram turnos. A suíte de testes
  original do enxame **não está neste repositório** (só `tests/test_cli.py`, 12/12 passando);
  o corpus dourado é o oráculo disponível. A cobertura cresce por construção nas próximas
  fases: qualquer entrada que divergir no diferencial (Fase 1+) entra no corpus. Expandir a
  cobertura por escavação dos gatilhos de cada módulo é trabalho da Fase 7.
- Uso previsto: a Fase 1 roda este corpus no runner diferencial (Rust × Python) comparando
  reply + módulo + digest de memória, agrupando diffs por módulo; a Fase 7 exige 100 % nesse
  corpus; a Fase 8 fixa o marco de paridade.

---

## 9. Como o turn acontece (mapa de execução)

```
pybot.py (CLI)                bot/runtime.py::turn(text, memory)
  parse args                  1. observe(): todos os módulos (16 têm), na ordem
  load bot (import ~4-7 s) ->     (-PRIORITY, nome); nunca responde; erros sileciosos
  loop stdin                 2. respond(): cada módulo com respond() (~97) roda sobre uma
                                CÓPIA da memória; retorna candidato {module, priority, reply}
                             3. arbiter.choose(text, memory, candidates) escolhe índice
                                (sem arbiter funcional → primeiro candidato)
                             4. a cópia do VENCEDOR vira a memória da conversa
                                (as ~96 cópias restantes são descartadas)
                             5. after(): módulos com after() (2) rodam no estado vencedor
                             6. runtime anexa {user, bot, module} a memory["turns"]
```

- `bot/modules/arbiter.py` (5 532 linhas, 183 funções): implementa por cima do "piso do
  candidato em ordem de PRIORITY" uma pilha de regras documentadas no docstring: especialista
  (`ANSWERS`) > generalista, recall band (PRIORITY 7000, linhas sem tópico próprio), nunca
  repetir a última resposta nem uma resposta inteira anterior, emergência (9000) nunca é
  passada por cima, luto repentino, "where do i start", `TAKES_AS_IT_COMES`, comida recusada,
  rule-outs. O Rust port mantém a mesma pontuação e desempates (Fase 7).
- `PRIORITY`: 103 módulos declaram; faixa de fallback −1000…−450 (greeting −1000,
  urgent −900, nudge −950), banda de conhecimento 0, chat 300, ANSWERS acima do 0,
  luto 2000–2100, safety 9000, observe do `understand.py` 10000.
- Módulo vencedor exposto em `result["module"]` e exibido pelo CLI com `-v`
  (`[module: %s | %.0f ms]`) — é o mesmo sinal que o corpus dourado grava.

---

## 10. Riscos, decisões e o que muda na Fase 2+

| Risco/achado | Consequência |
|---|---|
| 9 812 usos de `\b` e 7 087 de classes `\w \d \s` | Fase 2 precisa de tabelas Unicode reais (pelo menos Latin-1 + Latin Extended-A/B + marcas combinantes NFKD); gerar via script dev a partir de `unicodedata`, como previsto |
| 683 lookahead + 412 lookbehind | Pike VM com lookaround e **orçamento de passos** (nunca backtracking catastrófico); todos de largura fixa verificados |
| 8 backreferences (7 `\1` + 1 `(?P=)`) | reescrever manualmente: são `(.)\1+` (colapsar runs, ex.: "soooo"→"soo"), `\b(\w+)(\s+\1\b)+` / `\b(\w+)\s+\1\b` (palavras duplicadas), `^(?P<thing>[a-z]+…)\s+not\s+…(?P=thing)…$` (correção "X not Y X Z" em `memory/ack.py`) |
| `.lower()` 6 047×; `.isalpha/.isdigit/.isalnum/.isspace/.isupper/.islower` ~320× | semântica Unicode, não ASCII: Rust `char::to_lowercase` é correto; predicados exigem tabelas próprias ou categorias exatas (`is_alphabetic` ≈ Alphabetic; `isdigit` ≠ `is_numeric` em detalhes — gerar tabelas) |
| NFKD + strip de marcas (2 sítios) | subset NFKD só para Latino (o PyBot normaliza para lookup; ambos sítios operam em palavras em inglês/latino) |
| `difflib.SequenceMatcher.ratio() ≥ 0.7` (1 sítio) | portar Ratcliff–Obershelp exato ou substituir com equivalência medida no corpus (Fase 2.2) |
| `Fraction(...).limit_denominator(64)` + `Decimal` HALF_UP (`maths.py`) | reimplementar `Fraction` mínima (gcd/lcm, aritmética e comparação) ou casos fechados dos usos (receitas); sem float aproximado — divergência silenciosa de arredondamento passa no diferencial só se medida |
| `id(singleton)` como chave (54 sítios) | Fase 3: IDs `u32` atribuídos em `build.rs`/tabelas, nunca endereços |
| `deepcopy` por módulo por turno (~97×/turno) | Rust: `Session` por valor + snapshots estruturais (`Cow`/clone-on-write) ou clonar só o necessário; a semântica "a cópia do perdedor é descartada" já diz que basta clonar quando um módulo **escreve** em memória — copiar sob demanda |
| `sorted(dict)` / `set` scratch | determinístico já no Python 3.7+ (inserção); Rust: ordem de inserção explícita |
| 1 082 call-sites de regex não resolvidos estaticamente | Frente real fica com o corpus dourado + Fase 2 diferencial da engine em `(padrão, texto)` extraído do próprio bot (regex_inventory.json serve de corpus) |
| `re.X`/VERBOSE em 29 call-sites | ignorar espaços/comentários no parser da engine própria |
| `"…" * 400`, entradas enormes, bytes NUL, RTL, CJK na entrada | CLI: UTF-8 estrito com substituto documentado (Fase 8); orçamento de tamanho no Fase 9 |
| Arbiter de 5,5 mil linhas com hooks por nome | Fase 7: tabela estática `(nome, prioridade, fn pointer)` + mesma ordem de tie-break; testar com corpus completo |
| Corpus dourado cobre só o que os prompts exercitam | cobertura medida por módulo vencedor (seção 8); ampliar probes por domínio na esteira do diferencial (Fase 1) |

---

## 11. Metas de desempenho (definidas nesta fase, para a Fase 10)

O `GOAL.md` pede metas numéricas na Fase 0. Com base no profiling (seção 7) e no fato de que
startup Python ≈ 7 s (compilação + import dos 22 MB de literais) para um bot que responde
regras puras:

| Métrica | Python (medido) | Meta Rust (Fase 10) |
|---|---:|---:|
| Startup (import até pronto para 1º turno) | 3,7–6,9 s (quente/frio) | **≤ 100 ms** (dados via `include_bytes!`, parse zero-cópia) |
| p50 por turno | 0,283 s | **≤ 10 ms** |
| p95 por turno | 0,573 s | **≤ 25 ms** |
| p99 por turno | **4,591 s** (recall sem nome) | **≤ 50 ms** |
| max por turno | 4,591 s | **≤ 250 ms**, nunca I/O nem panic |
| RSS após 1 000 turnos | 212 MB após 100 turnos (pico; +85 MB em 100 turnos — `memory["turns"]` cresce sem limite no original) | **≤ 64 MB em estado estável**, com limite explícito de histórico (Fase 5) e plano no soak (Fase 9) |
| Fuzz/soak | — | 0 panics, 0 timeouts (Fase 9) |
| Equivalência no corpus dourado | — | ≥ 99,9 % (Fase 8); divergências documentadas e aprovadas |

Qualquer otimização depois do marco de paridade exige benchmark antes/depois e o diferencial
em 100 %.

---

## 12. Artefatos versionados

| Artefato | O que é |
|---|---|
| `tools/recon/gen_corpus.py` | gerador do corpus dourado (determinístico, `PYTHONHASHSEED=0`) |
| `tools/recon/verify_corpus.py` | re-roda amostra do corpus e compara byte a byte (spot-check de determinismo) |
| `tools/recon/inventory.py` | inventários estáticos (AST): imports, regex, dinamismo, não-determinismo |
| `tools/recon/inventory.json` | resultados brutos (imports, LOC, dinamismo, …) |
| `tools/recon/regex_inventory.json` | 11 318 padrões únicos com features e contagens — corpus de teste da Fase 2 |
| `tools/recon/profile_bot.py` | profiling (cProfile + latências + RSS) |
| `tools/recon/profile.json` | resultados brutos |
| `tests/golden/corpus.jsonl` | **corpus dourado**: um JSON por turno |
| `tests/golden/manifest.json` | versão, seed, contagem, sha256 do corpus |
