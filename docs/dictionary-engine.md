# Engine de dicionários locais

Lookitan 0.2 implementa leitores próprios ligados a um índice e a uma apresentação comuns.
O REA foi usado para identificar contratos e examinar evidências do Lookupper 2.0.6.0.
Não foi recuperado o fonte de sua engine NativeAOT: este trabalho reproduz comportamentos
com uma implementação independente, e não afirma equivalência de todos os algoritmos.

## Adicionar um dicionário

Em **Dicionários → Adicionar**, selecione o arquivo principal abaixo. Mantenha os arquivos
companheiros e as pastas de recursos ao lado dele. Também é possível selecionar um ZIP que
contenha um dicionário; o importador o extrai para uma pasta própria do aplicativo.
Quando um ZIP contém vários dicionários independentes, extraia-o e adicione cada principal.

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

O nome `.json` em Kaikki significa JSONL, um objeto por linha, não um único array gigante.
Os bancos `term_meta_bank`/`kanji_meta_bank` (frequência, acento tonal e IPA), que o Lookupper
mostra como *Frequency* e *Pitch accent*, viram um artigo por linha; assim dicionários só de
frequência ou só de acento também podem ser adicionados.

## Busca, memória e recursos

- A grafia consultada tem prioridade sobre a forma normalizada ou flexionada. A primeira forma
  encontrada em cada dicionário vence; homógrafos dessa forma são preservados, até 128 resultados.
- Os artigos seguem a ordem configurada, inclusive quando fontes locais e IA estão intercaladas.
  A exportação para o Anki agrupa todos os homógrafos da fonte, sem duplicar o mesmo artigo.
- Os leitores importados emitem um artigo por vez em SQLite. A consulta usa um índice de palavras
  e não materializa o dicionário inteiro. O cache SQLite de cada conexão é limitado a 512 KiB.
- MDict mantém os arquivos mapeados e descomprime somente os blocos usados. Seu cache de blocos
  tem limite de 4 MiB por arquivo, com até oito blocos. Artigos e mídia que atravessam blocos são
  concatenados corretamente. Os índices `.lki` persistem apenas chaves/offsets/ordem.
- Índices incompletos, corrompidos ou cuja fonte mudou são reconstruídos. A identidade usa
  caminho, tamanho e mtime; MDict também valida o cabeçalho e o checksum do sidecar. Ela não
  equivale a um hash criptográfico de todos os bytes do dicionário original a cada inicialização.
- Índices SQLite e ZIPs extraídos ficam em `dictionaries/` na pasta de dados do aplicativo.
  Os originais não são alterados. Remover uma fonte da lista não apaga seu arquivo original.
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
de kana. Português possui regras conservadoras de plural e `-mente`. Os outros idiomas
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
- GCIDE aceita a distribuição XML, não o SGML bruto `CIDE.*`. WordNet não implementa navegação
  do grafo semântico. Catálogos, downloads e servidores do Lookupper não são dependências.
- A importação de arquivos compactados gerais é ZIP; gzip/bzip2 são aceitos pelos leitores
  indicados. RAR, 7z e arquivos multipart não são extraídos automaticamente.

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
de referência e testes de integração dos leitores com SQLite/ZIP/recursos. Eles rodam em
Windows e Linux no workflow `dictionary-engine`. O harness compila os módulos reais do app
sem carregar Tauri/OCR, e o workflow principal ainda compila o aplicativo completo.
Os testes JavaScript exercitam ordem das fontes, todos os homógrafos no Anki e áudio/links.
Não se executam builds Rust neste PC, conforme `AGENTS.md`.

Referências dos formatos: [MDict](https://github.com/zhansliu/writemdict/blob/master/fileformat.md),
[Yomitan](https://github.com/yomidevs/yomitan/tree/master/ext/data/schemas),
[StarDict](https://github.com/huzheng001/stardict-3/blob/master/dict/doc/StarDictFileFormat),
[Slob](https://github.com/itkach/slob#slob-file-format),
[GCIDE XML](https://www.ibiblio.org/webster/). Fontes e hashes das transformações estão em `data/README.md`.
