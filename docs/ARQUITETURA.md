# Documentação de Arquitetura

Este documento fornece informações técnicas detalhadas sobre a arquitetura do sistema de detecção de fraude Rinha Rust.

## Visão Geral do Sistema

O sistema é uma API HTTP de alta performance para detecção de fraude que usa busca de k-vizinhos mais próximos (k-NN) em vetores de características quantizados. É projetado para operação de baixa latência com tempos de resposta sub-milissegundo.

### Princípios de Design Chave

1. **Operações zero-copy** - Índices mapeados em memória, passagem de descritores de arquivo
2. **Aceleração SIMD** - AVX2 para cálculos de distância vetorizados
3. **E/S orientada a eventos** - Loop de eventos baseado em epoll para manipulação eficiente de sockets
4. **Parsers especializados** - Parsing JSON rápido com múltiplas estratégias de fallback
5. **Indexação aprendida** - Particionamento de árvore de decisão para busca eficiente

## Arquitetura de Módulos

### Camada API (`src/api/`)

A camada API lida com requisições HTTP e coordena com o mecanismo de detecção de fraude.

#### `warmup.rs`
- Carrega e aquece o índice antes de servir requisições
- Suporta payloads de warmup externos e self-warmup
- Executa consultas para popular caches de CPU e tabelas de páginas
- Configurável via `RINHA_WARMUP_QUERIES` e variáveis de ambiente relacionadas

#### `server.rs`
- Servidor em modo FD para deployment de produção
- Recebe descritores de arquivo pré-configurados do load balancer
- Usa epoll para manipulação eficiente de eventos
- Suporta busy polling para latência reduzida sob carga

#### `handler.rs`
- Handler de requisições HTTP
- Faz parsing do corpo da requisição em vetor de consulta
- Chama o mecanismo de predição de fraude
- Formata e envia resposta

### Camada HTTP (`src/http/`)

#### `parser.rs`
- Parsing de requisições HTTP sem alocação
- Faz parsing da linha de requisição, cabeçalhos e corpo
- Valida content-length e path
- Suporta requisições em pipeline

#### `responses.rs`
- Modelos de resposta HTTP pré-formatados
- Inclui respostas de sucesso (200) e erro (400, 500)
- Otimizado para geração rápida de resposta

### Camada de Passagem de FD (`src/fd_passing/`)

Esta camada lida com comunicação entre processos via sockets de domínio Unix e passagem de descritores de arquivo.

#### `evented.rs`
- Coordenador principal do loop de eventos
- Gerencia instância epoll e registro de eventos
- Coordena operações de conexão, epoll e E/S

#### `conn.rs`
- Gerenciamento de estado de conexão
- Rastreia estado de leitura/escrita
- Gerencia ciclo de vida da conexão

#### `epoll.rs`
- Funções wrapper de epoll
- Registro/desregistro de eventos
- Manipulação de eventos edge-triggered
- Suporte a busy polling

#### `io.rs`
- Operações de E/S de baixo nível
- Leitura greedy para eficiência
- Configuração de socket (TCP_NODELAY, não-bloqueante)

### Camada de Índice (`src/index/`)

A camada de índice implementa o mecanismo de busca de similaridade de vetores.

#### `mod.rs`
- Ponto de entrada principal do índice
- Struct `SpecialistIndex` - carrega e consulta o índice
- Gerenciamento de partição e lookup de chave
- API de predição de fraude

#### `build.rs`
- Lógica de construção do índice
- Estrutura de dados de referência
- Serialização do índice para disco

#### `format.rs`
- Definições de formato do índice
- Estrutura de cabeçalho
- Versionamento de formato (V5)

#### `layout.rs`
- Acessores de layout de memória
- Acesso a partição, nó e vetor
- Operações de ponteiro unsafe para acesso zero-copy

#### `partition_scheme.rs`
- Particionamento de árvore aprendida
- Aprendizado de predicados de árvore a partir de consultas de amostra
- Computação de chave a partir de vetores de consulta
- Suporta Tree256 (árvore de 8 níveis)

#### `mmap.rs`
- Operações de mapeamento de memória
- Struct `MmapRegion` para gerenciar memória mapeada
- Otimizações específicas de plataforma (huge pages do Linux)

#### `search.rs`
- Algoritmos de busca k-NN
- `PendingSubtrees` para otimização de deferimento de label
- Cálculo de distância acelerado por AVX2
- Scanning de folha com SIMD
- Funções auxiliares para ordenação e inserção

### Camada de Vetor (`src/vector/`)

A camada de vetor faz parsing de payloads JSON em vetores de características quantizados.

#### `mod.rs`
- Entrada principal de parsing de vetor
- Enum `ParseError`
- Função `parse_query` com fallback multi-estratégia

#### `helpers.rs`
- Função de quantização
- Função de hash para IDs de comerciante
- Parsing de MCC e pontuação de risco
- Parsing de data/hora
- Parsing rápido de f64
- Funções de leitura de valores JSON

#### `compact.rs`
- Parser JSON compacto ordenado
- Assume ordem fixa de campos
- Caminho de parsing mais rápido
- Parsing direto em nível de byte

#### `single_pass.rs`
- Parser JSON de passagem única
- Lida com qualquer ordem de campos
- Pula campos desconhecidos
- Valida campos obrigatórios

#### `serde_fallback.rs`
- Parser JSON baseado em serde
- Suporte completo de desserialização
- Usado quando parsers especializados falham
- Lida com matching de campos case-insensitive

### Camada de Runtime (`src/runtime.rs`)

- Parsing de variáveis de ambiente
- Constantes de configuração
- Validação em build-time

## Estruturas de Dados

### Vetor de Consulta

```rust
pub type QueryVector = [i16; PACKED_DIMS]; // 16 dimensões
```

- Quantizado para i16 para eficiência de cache
- Preenchido para 16 para alinhamento AVX2
- Constante SCALE para quantização (build-time)

### Estrutura do Índice

```
SpecialistIndex {
    _mapping: MmapRegion,                    // Arquivo mapeado em memória
    reference_count: usize,                 // Número de referências
    partitions_base: *const u8,             // Dados de partição
    partition_count: usize,                 // Número de partições
    key_to_partition: [i32; 1024],          // Tabela de lookup de chave
    active_keys: Vec<u32>,                  // Chaves de partição ativas
    partition_scheme: PartitionScheme,      // Predicados de árvore
    nodes_base: *const u8,                  // Dados de nó
    node_count: usize,                      // Número de nós
    vectors: *const i16,                    // Dados de vetor
    vectors_len: usize,                     // Comprimento dos dados de vetor
    labels: *const u8,                      // Dados de label
    labels_len: usize,                      // Comprimento dos dados de label
    ref_indices: *const u32,                // Índices de referência
    ref_indices_len: usize,                 // Comprimento dos índices de referência
    node_class_bits: *const u8,             // Bits de classe para deferimento
    early_exit_threshold: i64,              // Limite de saída antecipada
    label_defer: bool,                      // Deferimento de label habilitado
}
```

### Partição

Cada partição contém:
- Chave (8 bits da travessia da árvore)
- Índice do nó raiz
- Caixa delimitadora (min/max por dimensão)
- Referência aos nós da árvore

### Nó

Cada nó da árvore contém:
- Índice do filho esquerdo
- Índice do filho direito
- Índice inicial no array de vetores
- Comprimento (número de vetores)

### Bloco de Vetor

Vetores são armazenados em blocos de 8 para eficiência AVX2:
- 8 vetores × 16 dimensões = 128 valores i16
- 8 labels (u8)
- 8 índices de referência (u32)

## Algoritmos

### Algoritmo de Busca k-NN

```
1. Fazer parsing da consulta em vetor quantizado
2. Computar chave de partição usando predicados da árvore
3. Obter lista de partições ativas
4. Para cada partição:
   a. Computar distância de limite inferior usando caixa delimitadora
   b. Se limite < k-ésima melhor distância:
      - Busca iterativa na árvore
      - Para nós folha:
        - Escanear vetores com AVX2
        - Atualizar k melhores vizinhos
      - Para nós internos:
        - Tentar deferir subárvore se consenso de label
        - Caso contrário, atravessar ambos os filhos
5. Reproduzir subárvores deferidas se consenso mudou
6. Retornar contagem de fraude dos k labels
```

### Otimização de Deferimento de Label

Durante a busca, se os k vizinhos atuais têm consenso (todos 0 ou todos 1):
- Deferir busca de subárvores que não contêm a classe de consenso
- Se consenso mudar mais tarde, reproduzir subárvores deferidas
- Reduz tempo de busca para casos claros

### Cálculo de Distância AVX2

```rust
// Computa distância euclidiana ao quadrado para 8 vetores em paralelo
fn scan_block_pair_avx2_bounded(
    vectors: &[i16],
    block_base: usize,
    q_pairs: &[__m256i; DIM_PAIRS],
    limit: i64,
) -> (u32, [i32; LANES])
```

- Pares de dimensões processados como vetores de 256 bits
- Subtração e multiplicação paralelas
- Soma horizontal para distância final
- Rejeição antecipada se distância excede limite

### Aprendizado de Particionamento de Árvore

```
1. Amostrar consultas dos dados de referência
2. Para cada nível da árvore:
   a. Para cada nó, encontrar melhor divisão:
      - Tentar cada dimensão
      - Encontrar limite que maximiza ganho de informação
   b. Armazenar predicado (dim, limite)
3. Usar predicados aprendidos para particionamento
```

## Otimizações de Performance

### Otimizações de Memória

1. **Mapeamento de Memória**
   - Índice carregado com `mmap` para zero-copy
   - `MADV_WILLNEED` para readahead
   - `MADV_HUGEPAGE` para eficiência de TLB (Linux)

2. **Quantização**
   - f64 → i16 quantização
   - Redução de 4x na memória
   - Melhor localidade de cache

3. **Layout de Bloco**
   - Vetores armazenados em blocos de 8
   - Alinhados para AVX2 (32 bytes)
   - Labels e índices intercalados

4. **Alocador**
   - mimalloc para fragmentação reduzida
   - Melhor performance multi-threaded

### Otimizações de CPU

1. **AVX2 SIMD**
   - 8 cálculos de distância em paralelo
   - Speedup de 4x sobre escalar
   - Apenas em x86_64 com suporte AVX2

2. **Busy Polling**
   - `EPOLL_BUSY_POLL` sob carga
   - Reduz trocas de contexto
   - Configurável via `RINHA_EPOLL_BUSY_POLL`

3. **Deferimento de Label**
   - Pular busca de subárvores irrelevantes
   - Reduz visitas de nó em ~30%
   - Configurável via `RINHA_LABEL_DEFER`

4. **Saída Antecipada**
   - Para quando k-ésima distância abaixo do limite
   - Configurável via `RINHA_EARLY_EXIT_THRESHOLD`
   - Reduz tempo de busca para predições confiantes

### Otimizações de E/S

1. **Epoll Edge-Triggered**
   - Notificação de eventos eficiente
   - Sem wakeups espúrios
   - Modo one-shot para fairness

2. **Sockets Não-Bloqueantes**
   - `TCP_NODELAY` para baixa latência
   - Modo não-bloqueante
   - Leitura greedy

3. **Passagem de Descritores de Arquivo**
   - Zero-copy entre processos
   - Sockets de domínio Unix
   - Dados ancilares SCM_RIGHTS

4. **Parsers Especializados**
   - Parser compacto para caso comum
   - Passagem única para flexibilidade
   - Fallback serde para compatibilidade

## Arquitetura de Deployment

### Instância Única

```
[Cliente] → [Servidor API]
            ↓
         [Índice]
```

### Multi-Instância com Load Balancer

```
[Cliente] → [Load Balancer] → [Servidor API 1]
                          → [Servidor API 2]
                          → [Servidor API N]
                          ↓
                        [Índice] (compartilhado via mmap)
```

O load balancer:
- Aceita conexões
- Passa FDs para processos worker
- Distribui carga entre instâncias
- Workers compartilham índice via mapeamento de memória

## Configuração de Build

### Perfil Release

```toml
[profile.release]
opt-level = 3           # Otimização máxima
lto = "fat"             # Otimização link-time
codegen-units = 1       # Unidade de codegen única para melhor otimização
panic = "abort"         # Abort no panic (sem unwinding)
strip = true            # Strip de símbolos
debug = 0               # Sem info de debug
overflow-checks = false # Desabilita verificações de overflow
```

### Configuração em Build-Time

- `RINHA_NATIVE_SCALE` - Escala de quantização (padrão: 1000)
- Validado em `build.rs`
- Falha build se inválido

## Variáveis de Ambiente

### Configuração do Índice

| Variável | Descrição | Padrão |
|----------|-----------|--------|
| `RINHA_INDEX_PATH` | Caminho para arquivo do índice | Obrigatório |
| `RINHA_NATIVE_SCALE` | Escala de quantização | 1000 (build-time) |
| `RINHA_EARLY_EXIT_THRESHOLD` | Limite de saída antecipada | 0 (desabilitado) |
| `RINHA_LABEL_DEFER` | Habilitar deferimento de label | 1 (habilitado) |

### Configuração de Runtime

| Variável | Descrição | Padrão |
|----------|-----------|--------|
| `RINHA_WARMUP_QUERIES` | Contagem de consultas de warmup | 1000 |
| `RINHA_SELF_WARMUP_URL` | URL de self-warmup | Nenhum |
| `RINHA_SELF_WARMUP_DURATION_MS` | Duração de self-warmup | 5000 |
| `RINHA_SELF_WARMUP_CONCURRENCY` | Concorrência de self-warmup | 4 |
| `RINHA_WARMUP_PAYLOADS_PATH` | Caminho de payloads de warmup | Nenhum |

### Configuração de Epoll

| Variável | Descrição | Padrão |
|----------|-----------|--------|
| `RINHA_EPOLL_BUSY_POLL` | Habilitar busy polling | 0 (desabilitado) |
| `RINHA_EPOLL_IDLE_US` | Timeout idle (μs) | 1000 |
| `RINHA_SPIN_BEFORE_BLOCK_US` | Duração de spin (μs) | 0 |

### Configuração de Socket

| Variável | Descrição | Padrão |
|----------|-----------|--------|
| `RINHA_CLIENT_FD_PRECONFIGURED` | FDs pré-configurados | 0 (falso) |

## Especificação do Formato do Índice

### Cabeçalho (V5)

```
Offset | Tamanho | Campo
-------|---------|-------
0      | 8       | Magic: "RNSPCST5"
8      | 4       | Scale (i32)
12     | 4       | Dimensões empacotadas (i32)
16     | 4       | Contagem de referências (i32)
20     | 4       | Contagem de partições (i32)
24     | 4       | Contagem de nós (i32)
28     | 4       | Contagem de blocos (i32)
32     | 2       | ID do esquema de partição (i16)
34     | 2       | Parâmetro do esquema de partição (u16)
36     | 2       | Contagem de cortes por nível (i16)
38     | 2       | Reservado
40     | N       | Predicados de árvore [dim: u8, flags: u8, threshold: i16]
```

### Seções de Dados

Após o cabeçalho, alinhado a limites de 4 bytes:

1. **Partições** (partition_count × 64 bytes)
   - Chave: u16 (2 bytes)
   - Raiz: u32 (4 bytes)
   - Min: [i16; 16] (32 bytes)
   - Max: [i16; 16] (32 bytes)
   - Padding: 6 bytes

2. **Nós** (node_count × 12 bytes)
   - Esquerda: i32 (4 bytes)
   - Direita: i32 (4 bytes)
   - Início: u32 (4 bytes)
   - Len: u16 (2 bytes)
   - Padding: 2 bytes

3. **Vetores** (block_count × 128 bytes)
   - 8 vetores × 16 dimensões = 128 valores i16

4. **Labels** (block_count × 8 bytes)
   - 8 labels (u8)

5. **Índices de Referência** (block_count × 32 bytes)
   - 8 índices (u32)

6. **Bits de Classe de Nó** (node_count bytes)
   - Máscara de classe para deferimento de label

## Testes

### Testes Unitários

Localizados em `tests.rs` de cada módulo ou blocos `#[cfg(test)]`:

- `src/index/tests.rs` - Testes de carregamento e busca do índice
- `src/vector/tests.rs` - Testes de parsing de vetor
- `src/http/parser/tests.rs` - Testes de parsing HTTP
- `src/api/tests.rs` - Testes de integração da API
- `src/lb/main.rs` - Testes do load balancer

### Executando Testes

```bash
# Todos os testes
cargo test

# Módulo específico
cargo test --lib index::tests

# Modo release (para testes de performance)
cargo test --release
```

## Características de Performance

### Latência

- **Alvo**: < 1ms latência p99
- **Típico**: 0.3-0.5ms
- **Fatores**: complexidade da consulta, estado do cache, carga de CPU

### Throughput

- **Instância única**: ~10k QPS
- **Multi-instância**: Escala linearmente com instâncias
- **Gargalo**: CPU (cálculos de distância AVX2)

### Memória

- **Tamanho do índice**: ~200MB para 1M referências
- **Overhead por instância**: ~50MB
- **mimalloc**: Melhor eficiência de memória

## Melhorias Futuras

Áreas potenciais para otimização:

1. **Aceleração por GPU** - Offload de cálculos de distância para GPU
2. **Melhor particionamento** - Esquemas aprendidos mais sofisticados
3. **Compressão** - Comprimir vetores em memória
4. **k adaptativo** - Ajustar k baseado na confiança da consulta
5. **Caching** - Cache de resultados de consultas recentes
