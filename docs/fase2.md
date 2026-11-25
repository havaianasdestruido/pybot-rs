# Fase 2 — estado desta entrega

> **Status: parcial — a Fase 2 não está fechada.** Este PR entrega os substitutos Unicode/RNG e
> uma fatia dos utilitários, mas não marca como concluído o escopo que ainda falta.

## Entregue

- `src/unicode.rs` + `src/unicode_tables.rs`: case-fold Unicode completo e tabelas versionadas
  (Unicode 14.0.0) para NFKD, marcas com classe combinante não-zero, `Nd` e categorias
  alfanuméricas. A transliteração latina explícita segue `bot/understanding/normalize.py`.
  O gerador de desenvolvimento é `tools/recon/gen_unicode_tables.py`; não há normalização,
  leitura de arquivo ou dependência Python no runtime.
- `src/rng.rs`: MT19937, seeding inteiro (incluindo limbs arbitrários), `random`, `getrandbits`
  até 128 bits, `_randbelow`, `choice`, `randrange`, `shuffle`, `sample`, snapshots/restore e
  API global sincronizada. `from_entropy` usa relógio do sistema e PID e não é criptográfica.
- `src/util.rs`: mapa com ordem de inserção, split/strip/replace/title/alnum limitados, formato
  de campos textuais, distância de Levenshtein e `SequenceMatcher.ratio` com orçamento e a
  heurística `autojunk` do Python.
- Oráculos versionados gerados pelo CPython: 260 seeds (incluindo negativos e inteiros grandes)
  para RNG e 3.969 pares exaustivos de strings curtas para `difflib`. Os scripts `--check`
  validam que os fixtures/tabelas não ficaram obsoletos. Os testes Rust estão em
  `tests/phase2_primitives.rs` e `src/{rng,unicode,util}.rs`.

## Ainda bloqueia a conclusão da Fase 2

1. **Regex (2.1): não implementada nesta entrega.** Faltam parser/VM Thompson ou Pike,
   capturas, grupos nomeados, flags, operações `search`/`match`/`fullmatch`/`finditer`/`sub`/
   `split`, lookaround de largura fixa com orçamento, `RegexSet` e comparação diferencial contra
   `re` nos padrões do inventário. As 8 regras com backreferences precisam de tratamento explícito.
2. **Utilitários (2.2): parcial.** Falta cobrir o formato numérico completo de `str.format`,
   templates/f-strings efetivamente usados e comparar a similaridade nos call-sites reais do
   bot. O ratio tem corpus diferencial curto exaustivo, não o corpus integral de textos da
   aplicação.
3. **Diferencial Rust:** os oráculos já estão versionados, mas a suíte `cargo test` precisa
   passar no CI; esta imagem de trabalho não contém `rustc`/`cargo`, então não foi possível
   executar o binário localmente nesta sessão.

Por isso, os checkboxes 2.1 e 2.2 continuam abertos e a saída da Fase 2 não pode ser declarada.
