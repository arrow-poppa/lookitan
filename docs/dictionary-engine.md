# Engine de dicionários locais

Lookitan 0.2 implementa leitores próprios ligados a um índice e a uma apresentação comuns.
O REA foi usado para identificar contratos e examinar evidências do Lookupper 2.0.6.0.
Não foi recuperado o fonte de sua engine NativeAOT: este trabalho reproduz comportamentos
com uma implementação independente, e não afirma equivalência de todos os algoritmos.

## Notes for agents (moved from AGENTS.md on 2026-11-11)

- **Local dictionaries (0.2):** the sections after this one (in Portuguese) give formats, evidence and limits.
  The settings wire tag `kind: mdict` is retained for all local readers to keep existing settings
  compatible; `DictInfo.format` identifies the actual format. Do not use this tag to infer file format.
  **Storage follows Lookupper's model (user's decision, 2026-10-15): no database.** Importing a ZIP
  unpacks it into `<local data>/dictionaries/content/<id>/` (`source::install_batch`/`source::install`; ZIP, RAR, 7z, TAR or gz/bz2/xz, `dictionaries/archive.rs`), and the files are
  read from there unchanged; removing the dictionary deletes that copy. A main file chosen directly
  is read in place. No dictionary holds its headwords in RAM: MDict keeps the keys of each .mdx and .mdd in `<file>.lkkeys` (since 2026-11-14; see "Chaves do MDict" below). Every other format
  keeps a headword index beside its files, `<main>.lkidx` (`dictionaries/lkidx.rs`: Lookupper's
  HeadwordIndex layout plus each entry's offset/length in the source; in the app cache for a file
  used in place), and renders the entry from the source bytes at lookup time: Yomitan
  (`yomitan.lkidx`, `dictionaries/yomitan.rs`) and the rest in `dictionaries/inplace.rs`, which
  re-runs the format's reader over the entry's span. `.dz` is read by chunk (`dictzip.rs`); plain
  gzip/bzip2 lexicon files are unpacked once into the cache. There is no SQLite any more. Lookupper's formats: mdict, yomitan, stardict, slob, dsl, cc-cedict, gcide,
  webster-text, wordnet-lmf-xml, jmdict-simplified-json, kanjidic2-json, kaikki-jsonl; Codex's
  other readers stay as extras. All
  source/AI sections follow one configured popup order. Anki retains all homographs per source.
  MDict block caches are bounded by bytes; do not restore a count-only cache. Imports/loads are
  serialized in AppState, with atomic index publication and safe ZIP extraction. Cleanup at start
  deletes only `.unpacking-*` folders and copies marked `.lookitan-removed`, never unmarked ones.
  Morphology uses the OCR language. Order of forms: on-screen spelling, then Hunspell stems
  (Lookupper's de/en/es/fr/pt/ru plus every other recognition language with a wooorm/dictionaries
  package: bg cs da el hr hu it la nb nl pl ro sk sl sr sr-latn sv tr uk; Bosnian uses hr; ar is AyaSpell from LibreOffice's dictionaries; fi and id have none;
  `dictionaries/hunspell.rs`; files in `resources/hunspell/` from
  `fetch-assets.py`, one language loaded at a time), then Yomitan rules for ja, en, es, ar, uk
  and de (zu-infinitive only; `data/*-yomitan.json` from yomidevs/yomitan d3c54c1, Arabic and
  some Ukrainian rules carry Yomitan's pattern, run with fancy-regex), German ge…t/ge…en
  participles written in morphology.rs, WordNet EN exceptions and conservative PT rules. Original Lookupper language packs/algorithms are not copied.
  Run reader tests on GitHub Actions (`dictionary-engine` / `scripts/dictionary-harness`), never
  compile Rust locally. Dependency lockfile resolution without compiling is allowed.

## Adicionar um dicionário

Em **Dicionários → Adicionar**, selecione o arquivo compactado do dicionário, como no Lookupper:
ZIP, RAR (RAR4/RAR5, também dividido em volumes `.part1.rar`… ou `.rar`/`.r00`…), 7z, TAR
(`.tar`, `.tar.gz`/`.tgz`, `.tar.bz2`, `.tar.xz`) ou um único arquivo `.gz`/`.bz2`/`.xz`. Ele é
extraído para uma pasta própria do aplicativo. Também é possível escolher o arquivo principal
abaixo; ele é lido onde está, com os arquivos companheiros ao lado. É possível selecionar vários
arquivos de uma vez. Se um pacote contém vários dicionários, todos são importados automaticamente,
sem seleção adicional. Cada um fica em uma pasta própria e pode ser removido separadamente,
apagando sua cópia física sem afetar os demais. No MDict, cada `.mdx` leva apenas os `.mdd`/`.N.mdd`
do mesmo nome-base. Recursos soltos potencialmente compartilhados são copiados para cada pasta.
Os MDX/MDD exclusivos são movidos da extração temporária, sem duplicar os arquivos grandes.

Cada arquivo selecionado é uma transação: se um principal não puder ser aberto, nenhum dicionário
daquele arquivo é cadastrado, e as pastas criadas são removidas. Os próximos arquivos selecionados
continuam sendo processados; sucessos e erros aparecem ao final. Falhas de MDD continuam como
avisos, conforme o comportamento anterior. Arquivos escolhidos diretamente são lidos no lugar e
nunca são apagados por uma falha ou remoção. Pacotes com um só dicionário mantêm o layout anterior.

A extração e a distribuição das cópias preservam limites de 32 GiB e 200 mil itens; os recursos
copiados para várias pastas contam novamente no orçamento da instalação. Durante a importação,
recursos compartilhados podem ocupar espaço temporário adicional; não há cópia integral em RAM.

| Formato | Arquivo principal | Conteúdo lido |
| --- | --- | --- |
| MDict 1.x/2.x | `.mdx` | HTML, cabeçalho de estilos, aliases, `.mdd`/`.N.mdd`, LZO/zlib/sem compressão |
| Yomitan | `.zip` ou `index.json` | Bancos de termos e kanji, frequência, acento tonal, IPA, leituras, tags, definições estruturadas e recursos |
| ABBYY DSL | `.dsl` ou `.dsl.dz` | UTF-8/UTF-16/codepages declaradas, aliases e marcação comum |
| StarDict | `.ifo` | `.idx`/`.idx.gz`, `.dict`/`.dict.dz`, `.syn`, tipos de conteúdo e pasta `res/` |
| Aard2/Slob | `.slob` | HTML e recursos em bins sem compressão/zlib/bzip2/RAW LZMA2 |
| CC-CEDICT | `.txt`/`.u8`, também gzip | Tradicional, simplificado, pinyin e definições |
| Kaikki/Wiktionary | `.jsonl`/`.json`, também `.jsonl.gz`/`.jsonl.bz2` | Registros JSONL, sentidos, formas e exemplos |
| JMdict/JMnedict | `.xml`, `.xml.gz` ou `.xml.bz2` | Grafias, leituras, sentidos e nomes |
| KANJIDIC2 | `.xml`, `.xml.gz` ou `.xml.bz2` | Caracteres, leituras e significados |
| Webster/GCIDE XML | `.xml` ou `gcide.xml` | Verbetes e fragmentos locais `gcide_a.xml`…`gcide_z.xml` |
| WordNet | `data.noun` ou `index.noun` | Arquivos `data.*`, sinônimos e exceções `.exc` da mesma pasta |
| jmdict-simplified (Lookupper) | `.json` com `"languages"` e `"dictDate"` no início | JMdict e JMnedict: grafias, leituras, sentidos, tags e referências |
| kanjidic2 JSON (Lookupper) | `.json` com `"characters"` e `"dictDate"` no início | Leituras on/kun/nanori, significados, traços, série, JLPT |
| WN-LMF (Lookupper) | `.xml` com `<LexicalResource` no início | Open English WordNet etc.: lemas, formas, definições, exemplos e sinônimos |
| Webster em texto (Lookupper) | `.txt` do Project Gutenberg (ebook 29765) | Verbetes do Webster 1913, um por cabeçalho em maiúsculas |
| GCIDE (Lookupper) | `CIDE.A` … `CIDE.Z` | Marcação SGML do GCIDE, entidades `<x/` e escapes `\'xx` |

Os formatos marcados "(Lookupper)" são as variantes que o Lookupper lê, reconhecidas como ele
reconhece (o mesmo teste nos primeiros bytes do arquivo). As demais leituras de JMdict,
KANJIDIC2, WordNet e Webster/GCIDE em XML são extras do Lookitan.

O nome `.json` em Kaikki significa JSONL, um objeto por linha, não um único array gigante.
Os bancos `term_meta_bank`/`kanji_meta_bank` (frequência, acento tonal e IPA), que o Lookupper
mostra como *Frequency* e *Pitch accent*, viram um artigo por linha; assim dicionários só de
frequência ou só de acento também podem ser adicionados.

## Busca, memória e recursos

- A grafia consultada tem prioridade sobre a forma normalizada ou flexionada. A primeira forma
  encontrada em cada dicionário vence; homógrafos dessa forma são preservados, até 128 resultados.
- Os artigos seguem a ordem configurada, inclusive quando fontes locais e IA estão intercaladas.
  A exportação para o Anki agrupa todos os homógrafos da fonte, sem duplicar o mesmo artigo.
- Como no Lookupper, não há banco de dados. Um ZIP importado é extraído para
  `dictionaries/content/<id>/` na pasta de dados local do app, e os arquivos são lidos de lá,
  sem conversão; remover o dicionário apaga essa cópia. Um arquivo escolhido diretamente é lido
  onde está.
- MDict guarda as chaves de cada `.mdx` e `.mdd` em `<arquivo>.lkkeys` (ao lado do arquivo na cópia do
  Lookitan; no cache do app para um arquivo usado no lugar), e descomprime só os blocos de
  verbetes usados (cache de até 4 MiB por arquivo). Ver "Chaves do MDict" abaixo.

### Chaves do MDict (2026-11-14, a pedido do usuário)

Antes, cada abertura lia todas as chaves do `.mdx` para a memória, uma alocação por chave
(~85 bytes cada), com limite de 2 milhões de chaves e 128 MiB. O ipa-dict (todas as línguas, 4.765.262
chaves) era recusado ("entry count exceeds the reader limit") e, sem o limite, custaria ~400 MiB de
RAM a cada início. Agora (`mdict.rs`, `KeyBuilder`/`KeyTable`):

- As chaves formam uma tabela plana: offsets dos registros, inícios únicos, fins dos textos, ordem
  por chave normalizada (estável entre duplicatas) e os textos em UTF-8. Não é banco de dados e o
  conteúdo não é convertido: os verbetes continuam sendo lidos do `.mdx`.
- Regra do usuário: todo dicionário tem índice em disco (SSD), nada de chaves na RAM a cada início.
- Para cada `.mdx` e `.mdd` a tabela vai para `<arquivo>.lkkeys` (cabeçalho `LKMDXKEY` com tamanho e
  data do arquivo, regras de chave e contagens), escrita uma vez (arquivo temporário + rename) e depois
  mapeada na memória. Cabeçalho diferente, tamanho errado ou arquivo truncado: é reconstruída.
  As leituras são todas verificadas, então um arquivo danificado dá respostas vazias, não falha.
- A construção usa até 1 GiB uma vez (`MAX_BUILD_BYTES`; ~250 MiB no ipa-dict); limite de 50
  milhões de chaves (`MAX_KEYS`, ordinais u32).
- Só quando a pasta não aceita escrita a tabela fica na memória (limite de 128 MiB).
- Remover um dicionário não deixa nada: a cópia em `content/<id>/` (com `.mdx`, `.mdd` e `.lkkeys`)
  é apagada, e um dicionário usado no lugar perde o que deixou no cache, `indexes/<id>.…` e
  `unpacked/<id>.…` (`remove_unused_cache` em lib.rs, também a cada início, sob o `dictionary_gate`,
  para o que o Windows ainda mantinha aberto). O arquivo original escolhido pelo usuário nunca é apagado.
- Os dicionários MDict já instalados ganham o `.lkkeys` na primeira abertura depois da atualização.
- Busca: busca binária na ordem normalizada, como o `.lkidx` dos outros formatos (o hoshidicts usa
  hash perfeito sobre arquivos convertidos; aqui o `.mdx` não é convertido).
- Os outros formatos guardam ao lado dos arquivos um índice de palavras, `<arquivo>.lkidx`
  (`yomitan.lkidx` no Yomitan), no layout do HeadwordIndex do Lookupper: palavras normalizadas em
  ordem, offsets e ids, mais onde está cada verbete no arquivo original. O índice é mapeado na
  memória; na consulta, só o verbete é lido do original e convertido em HTML. Arquivos `.dz` são
  lidos por bloco; gzip/bzip2 sem blocos são descompactados uma vez para o cache.
- Índices incompletos, corrompidos ou cuja fonte mudou (tamanho ou data de cada arquivo) são
  reconstruídos. Os originais nunca são alterados.
- Recursos soltos aceitam subpastas, separadores Windows e nomes com caixa diferente no Linux.
  Caminhos que escapam da pasta do dicionário são recusados; o mesmo vale para ZIPs com traversal
  ou links simbólicos. A extração é limitada a 32 GiB e 200 mil membros.
- Áudio de verbetes interrompe o áudio anterior; fechar o popup interrompe a reprodução.
  `entry://` e `bword://` fazem outra consulta, preservando âncoras. Estilos, imagens e scripts
  locais são fornecidos pelo protocolo `lkt`, em iframes sem acesso aos comandos Tauri.

O parser possui limites explícitos de tamanho de bloco/artigo/linha. Dicionários com unidades
excepcionalmente grandes produzem um erro, em vez de uma alocação sem limite neste PC.

## Formas de dicionário

O idioma de OCR seleciona as transformações. A ordem das formas tentadas é: a grafia da tela
(e suas normalizações), depois as formas de dicionário do **Hunspell**, depois as regras abaixo.
Como o Lookupper, o Hunspell cobre alemão, inglês, espanhol, francês, português e russo, com os
mesmos arquivos `.aff`/`.dic` que ele distribui: os pacotes `dictionary-*` do wooorm/dictionaries
(conferidos byte a byte, exceto quebras de linha), baixados da fonte original pelo
`scripts/fetch-assets.py` e instalados em `hunspell/`. Só o idioma do OCR fica carregado.

Inglês utiliza ainda regras literais do Yomitan e
exceções lexicais do WordNet 3.0; japonês utiliza regras condicionais do Yomitan e normalização
de kana. Espanhol, árabe e ucraniano usam as regras do Yomitan (as do árabe e parte das do
ucraniano com os padrões dele), e alemão as de zu-infinitivo mais particípios ge…t/ge…en
escritos no Lookitan. Português possui regras conservadoras de plural e `-mente`. Os outros idiomas
recebem normalização de grafia, sem aplicar indevidamente sufixos ingleses ou portugueses.
As regras são limitadas a 256 candidatos, 1.024 estados e 12 passos por consulta.

Isso não reproduz os packs Sudachi/coreano/chinês do Lookupper. A seleção de prefixos
CJK da camada OCR é mantida; não há promessa de equivalência com sua segmentação lexical.
Os dados GPL do Yomitan e a licença WordNet estão em `src-tauri/src/dictionaries/data/`.

## Aparência dos verbetes (2026-11-14, a pedido do usuário)

Todo verbete fica sobre o fundo do próprio pop-up, em qualquer formato e tema (o usuário pediu que os
dicionários "se misturassem" ao pop-up como no Yomitan).

- **Estilo da página:** `src/lib/dictionaries/entry.css` (incluído por `protocol.rs`). Duas aparências: `lkt-themed` (formatos que o
  Lookitan monta) e `lkt-own` (MDict e Slob, que trazem CSS próprio). Nenhuma pinta página nem
  inverte cores: o filtro `invert` antigo sobre a página branca do MDict/Slob foi removido.
- **Cores reescritas** (`dictionaries/colors.rs`), ao servir o verbete e cada `.css` do dicionário:
  - cada cor vira `light-dark(original, escura)`, e a declaração original fica antes, para WebViews sem `light-dark()`;
  - texto escuro ganha a mesma matiz clara (luminosidade HSL entre 0,62 e 0,92), e fundos quase brancos (≥ 0,85) ficam transparentes;
  - fundos claros coloridos viram um tom escuro da mesma matiz;
  - imagens não são tocadas;
  - vale para `<style>`, `style=`, `color=`, `bgcolor=`, e não toca seletores, `url()`, `var()`, textos e comentários.
  A exportação para o Anki usa o HTML original (`entry_html`), sem a reescrita.
- **Layout comum:** classes e regras do Yomitan (`display.css`, `structured-content.css`; crédito em `NOTICE.md`):
  `entry`, `headword`/`headword-term`/`headword-reading`, `tag` com `data-category` (cores das categorias do Yomitan),
  `definition-list`/`definition-item`, `gloss-list`/`gloss-item`/`gloss-content` (sem marcador quando há um só), e
  `gloss-sc-<tag>` em cada elemento do conteúdo estruturado (tabela dentro de `gloss-sc-table-container`).
  Partes próprias do Lookitan para os outros formatos: `lk-example`, `lk-example-translation`, `lk-note`, `lk-xref`, `lk-level`.
- **Yomitan:** as quebras de linha do glossário viram `<br>`, como o Yomitan as mostra (`white-space: pre-line`).
  O MacMillan + IPA-v3 estrutura cada verbete só com linhas. Até 2026-11-14 elas eram unidas num parágrafo, como faz o Lookupper.
  As tags do `tag_bank` guardam a categoria (`TagMeta`).
- **Os outros formatos no layout comum** (etapa 2): cada leitor monta o verbete com as peças de `containers.rs`
  (`headword_html`, `tag_pill`, `definition_list`, `gloss_list`, `example_html`, `note_html`, `xref_html`), até onde os dados permitem:
  - CC-CEDICT: pinyin como leitura, forma tradicional como nota, glosas em lista;
  - Kaikki: IPA como leitura, classe e língua como tags, etimologia como nota, sentidos numerados com exemplos e traduções;
  - JMdict/JMnedict (XML e JSON): leituras ao lado, outras grafias como nota, classes como tags coloridas, cada uma separada;
  - KANJIDIC: leituras, significados e estatísticas como no kanji do Yomitan;
  - WordNet (texto e LMF): classe como tag, um sentido por synset, exemplos, sinônimos como referência;
  - DSL: a palavra no topo (o corpo DSL não a repete), `[p]` como tags, `[ex]` exemplos, `[com]` notas, `[m1]`–`[m9]` como `lk-level`;
  - StarDict: a palavra no topo, a não ser que o artigo já comece com ela; no XDXF, `<ex>`, `<co>` e `<abr>` viram exemplo, nota e tag;
  - GCIDE: `<pos>`/`<fld>`/`<mark>` como tags, exemplos e citações como `lk-example`, etimologia e autores como notas;
  - Webster (texto): `Note:` como nota, `Syn.` como referência;
  - MDict e Slob mantêm o próprio HTML (só as cores mudam); o Slob em texto puro ganha a palavra no topo.

## Limites de compatibilidade

- MDict v3 e dicionários que exigem registro (`Encrypted=1`) são rejeitados explicitamente.
  Os envelopes Adler32 externos conservam a política do leitor anterior; a descompressão zlib,
  tamanhos, índices e offsets são validados.
- DSL suporta a marcação comum, não o formato binário `.lsd` nem arquivos de mídia internos
  proprietários. StarDict não lê resourceDB `.rifo/.ridx/.rdic`; use recursos na pasta `res/`.
- Slob precisa estar em um arquivo físico; partes divididas devem ser reunidas previamente.
  Referências a fragmentos preservam o artigo completo. Aliases podem repetir HTML no índice.
- WordNet não implementa navegação do grafo semântico. Catálogos, downloads e servidores do Lookupper não são dependências.
- RAR e 7z protegidos por senha não são suportados.

## Evidências e validação

Alvo instalado: pacote `41543AlekseiVekhov.1221837D05F64`, versão 2.0.6.0, x64.
SHA-256 de `Lookupper.exe`: `16af03f18ddc75547683895c2d59bc25d053a311626710b51aaebe9547c7d5e8`.

| Evidência | Conclusão e alcance |
| --- | --- |
| REA `ev_b7583bbd057a4d91ac55410653e348cc10d8e3428d593b3a17a4ed079493a772` | Identidade do PE e ausência de metadados/CIL; não é o fonte recuperado da engine |
| REA `ev_44a5dc9d494eec669bd1ffa6dc194686eb0f8177f65f3cec5fdfcba3fa2ab9b0` | Análise estática dos scripts embarcados, incluindo comportamento de áudio de artigos |
| Vista REA `ev_dacb146a4ab957eca11b3815f8c10a70e14bc3a9b84b115129cf0ef2c3d3d367` | Reutilizada via CLI, preservando cobertura parcial e falha de heap em Dark Reader |
| Strings auxiliares, não Evidence REA | Nomes `IDictionaryReader`, `DictionaryEngine`, `HeadwordIndex`, `TryLoadIndex`, `SaveIndex` e leitores por formato sustentam a arquitetura proposta; não comprovam seus algoritmos |

O engine é validado com arquivos sintéticos completos, um Slob gerado pelo Writer público
de referência e testes de integração dos leitores com índices .lkidx/ZIP/recursos. Eles rodam em
Windows e Linux no workflow `dictionary-engine`. O harness compila os módulos reais do app
sem carregar Tauri/OCR, e o workflow principal ainda compila o aplicativo completo.
Os testes JavaScript exercitam ordem das fontes, todos os homógrafos no Anki, áudio/links e a
seleção de vários arquivos, execução sequencial e resultados mistos na importação. O harness
Rust cobre pacotes 7z com dois MDX e MDDs numerados, recursos, reabertura, exclusão independente,
rollback e descoberta de formatos mistos. O workflow `dictionary-engine` aceita opcionalmente
`real_mdict_archive_url` no disparo manual para validar o pacote real com dois MDX no Linux;
o arquivo baixado permanece no diretório temporário do runner e não é incluído no repositório.
Não se executam builds Rust neste PC, conforme `AGENTS.md`.

Referências dos formatos: [MDict](https://github.com/zhansliu/writemdict/blob/master/fileformat.md),
[Yomitan](https://github.com/yomidevs/yomitan/tree/master/ext/data/schemas),
[StarDict](https://github.com/huzheng001/stardict-3/blob/master/dict/doc/StarDictFileFormat),
[Slob](https://github.com/itkach/slob#slob-file-format),
[GCIDE XML](https://www.ibiblio.org/webster/). Fontes e hashes das transformações estão em `data/README.md`.
