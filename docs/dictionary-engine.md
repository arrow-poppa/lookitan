# Engine de dicionários locais

Lookitan 0.2 implementa leitores próprios ligados a um índice e a uma apresentação comuns.
O REA foi usado para identificar contratos e examinar evidências do Lookupper 2.0.6.0.
Não foi recuperado o fonte de sua engine NativeAOT: este trabalho reproduz comportamentos
com uma implementação independente, e não afirma equivalência de todos os algoritmos.

## Adicionar um dicionário

Em **Dicionários → Adicionar**, selecione o arquivo compactado do dicionário, como no Lookupper:
ZIP, RAR (RAR4/RAR5, também dividido em volumes `.part1.rar`… ou `.rar`/`.r00`…), 7z, TAR
(`.tar`, `.tar.gz`/`.tgz`, `.tar.bz2`, `.tar.xz`) ou um único arquivo `.gz`/`.bz2`/`.xz`. Ele é
extraído para uma pasta própria do aplicativo. Também é possível escolher o arquivo principal
abaixo; ele é lido onde está, com os arquivos companheiros ao lado. Quando um arquivo compactado
contém vários dicionários independentes, extraia-o e adicione cada principal.

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
- MDict lê o próprio índice de chaves do `.mdx` a cada abertura, sem arquivo extra, e descomprime
  só os blocos usados (cache de até 4 MiB por arquivo).
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
Os testes JavaScript exercitam ordem das fontes, todos os homógrafos no Anki e áudio/links.
Não se executam builds Rust neste PC, conforme `AGENTS.md`.

Referências dos formatos: [MDict](https://github.com/zhansliu/writemdict/blob/master/fileformat.md),
[Yomitan](https://github.com/yomidevs/yomitan/tree/master/ext/data/schemas),
[StarDict](https://github.com/huzheng001/stardict-3/blob/master/dict/doc/StarDictFileFormat),
[Slob](https://github.com/itkach/slob#slob-file-format),
[GCIDE XML](https://www.ibiblio.org/webster/). Fontes e hashes das transformações estão em `data/README.md`.
