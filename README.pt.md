# Rinha Rust - Detecção de Fraude com Baixa Latência

[![Rust CI](https://github.com/fksegundo/rinha-rust-tree/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/fksegundo/rinha-rust-tree/actions/workflows/rust-ci.yml)
[![Build image](https://github.com/fksegundo/rinha-rust-tree/actions/workflows/publish-image.yml/badge.svg)](https://github.com/fksegundo/rinha-rust-tree/actions/workflows/publish-image.yml)
[![GHCR image](https://img.shields.io/badge/GHCR-rinha--rust--tree--api-blue)](https://github.com/fksegundo/rinha-rust-tree/pkgs/container/rinha-rust-tree-api)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Implementação em Rust de alta performance para o desafio [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026), com detecção de fraude de baixa latência usando busca de similaridade de vetores.

[Read in English](README.md)

## Visão Geral

Este projeto implementa uma API de detecção de fraude que usa busca de k-vizinhos mais próximos (k-NN) em vetores de características quantizados. O sistema é otimizado para baixa latência através de:

- **AVX2 SIMD** para cálculos de distância vetorizados
- **Índices mapeados em memória** para carregamento zero-copy
- **Loop de eventos baseado em epoll** para multiplexação de E/S eficiente
- **Passagem de descritores de arquivo** para comunicação entre processos
- **Particionamento com árvore aprendida** para busca eficiente no índice
- **Parsers JSON especializados** para parsing rápido de requisições

## Arquitetura

### Componentes Principais

- **`src/api/`** - Servidor HTTP API com warmup e tratamento de requisições
- **`src/http/`** - Parsing de requisições/respostas HTTP
- **`src/fd_passing/`** - Passagem de descritores de arquivo com loop de eventos epoll
- **`src/index/`** - Índice de vetores com particionamento baseado em árvore
- **`src/vector/`** - Parsing de vetores de consulta com múltiplas estratégias
- **`src/runtime/`** - Configuração de runtime a partir de variáveis de ambiente
- **`src/lb/`** - Load balancer para deployment multi-instância

### Organização de Módulos

```
src/
├── api/
│   ├── mod.rs          # Ponto de entrada principal
│   ├── warmup.rs       # Lógica de warmup do índice
│   ├── server.rs       # Servidor em modo FD
│   └── handler.rs      # Handler de requisições HTTP
├── http/
│   ├── mod.rs          # Entrada do módulo HTTP
│   ├── parser.rs       # Parsing de requisições
│   └── responses.rs    # Constantes de resposta
├── fd_passing/
│   ├── mod.rs          # Entrada de passagem de FD
│   ├── evented.rs      # Lógica de loop de eventos
│   ├── conn.rs         # Gerenciamento de conexões
│   ├── epoll.rs        # Operações de epoll
│   └── io.rs           # Utilitários de E/S
├── index/
│   ├── mod.rs          # Entrada do índice
│   ├── build.rs        # Construção do índice
│   ├── format.rs       # Formato do índice
│   ├── layout.rs       # Layout de memória
│   ├── partition_scheme.rs  # Particionamento de árvore
│   ├── mmap.rs         # Mapeamento de memória
│   └── search.rs       # Algoritmos de busca
├── vector/
│   ├── mod.rs          # Entrada de parsing de vetores
│   ├── helpers.rs      # Funções auxiliares
│   ├── compact.rs      # Parser compacto ordenado
│   ├── single_pass.rs  # Parser de passagem única
│   └── serde_fallback.rs  # Parser fallback com serde
└── runtime.rs          # Configuração de runtime
```

## Tecnologias

- **Rust 2024 Edition** - Rust moderno com as últimas funcionalidades da linguagem
- **libc** - Chamadas diretas ao sistema para epoll, mmap, operações de socket
- **AVX2 SIMD** - Cálculos de distância vetorizados via `std::arch::x86_64`
- **mimalloc** - Alocador de memória de alta performance
- **serde_json** - Parsing JSON fallback
- **threadpool** - Pool de threads para operações concorrentes
- **flate2** - Suporte a compressão

## Algoritmos

### Busca k-NN com Particionamento de Árvore

O índice usa uma árvore de decisão aprendida para particionar o espaço vetorial em 256 buckets (Tree256). Cada consulta é roteada para as partições mais relevantes baseado em predicados da árvore, então a busca k-NN é realizada dentro dessas partições.

**Características principais:**
- **Otimização de deferimento de label** - Pula busca de subárvores quando consenso é alcançado
- **Limite de saída antecipada** - Para busca quando distância do k-ésimo vizinho está abaixo do limite
- **Distância acelerada por AVX2** - Computa 8 distâncias em paralelo usando SIMD
- **Poda por limite inferior** - Usa caixas delimitadoras para pular partições irrelevantes

### Esquema de Particionamento

O esquema de particionamento é aprendido a partir de consultas de amostra usando uma árvore de decisão:
- **Profundidade da árvore**: 8 níveis (configurável até 10)
- **Predicados**: Limites aprendidos em dimensões de vetores
- **Computação de chave**: Travessia binária produz chave de partição de 8 bits

### Estratégias de Parsing JSON

Três estratégias de parsing são tentadas em ordem de performance:

1. **Parser Compacto Ordenado** - Assume ordem fixa de campos, caminho mais rápido
2. **Parser de Passagem Única** - Lida com qualquer ordem de campos com skipping
3. **Fallback Serde** - Desserialização completa com serde para compatibilidade

### Cálculo de Distância

Distância euclidiana ao quadrado computada com AVX2:
- Pares de dimensões processados em paralelo
- Rejeição antecipada usando limites de distância
- Valores quantizados i16 para eficiência de cache

## Variáveis de Ambiente

### Configuração do Índice
- `RINHA_INDEX_PATH` - Caminho para o arquivo do índice
- `RINHA_NATIVE_SCALE` - Escala de quantização (build-time)
- `RINHA_EARLY_EXIT_THRESHOLD` - Para busca quando k-ésima distância está abaixo deste valor
- `RINHA_LABEL_DEFER` - Habilita otimização de deferimento de label (0/1)

### Configuração de Runtime
- `RINHA_WARMUP_QUERIES` - Número de consultas de warmup
- `RINHA_SELF_WARMUP_URL` - URL para self-warmup
- `RINHA_SELF_WARMUP_DURATION_MS` - Duração do self-warmup
- `RINHA_SELF_WARMUP_CONCURRENCY` - Concorrência do self-warmup
- `RINHA_WARMUP_PAYLOADS_PATH` - Caminho para payloads de warmup

### Configuração de Epoll
- `RINHA_EPOLL_BUSY_POLL` - Habilita busy polling (0/1)
- `RINHA_EPOLL_IDLE_US` - Timeout idle em microsegundos
- `RINHA_SPIN_BEFORE_BLOCK_US` - Duração de spin antes de bloquear

### Configuração de Socket
- `RINHA_CLIENT_FD_PRECONFIGURED` - Assume que FDs estão pré-configurados (0/1)

## Build

```bash
# Build de binários release
cargo build --release --bin api --bin preprocess --bin lb

# Build de imagens Docker
make build

# Valida configuração do Docker Compose
make config
```

## Execução

```bash
# Inicia stack local
make up

# Para stack local
make down
```

## Testes

```bash
# Executa todos os testes
cargo test

# Executa teste específico
cargo test --lib vector::tests::tests::compact_ordered_matches_serde_fallback
```

## Otimizações de Performance

### Memória
- **Índices mapeados em memória** - Carregamento zero-copy com `mmap`
- **Advice de huge page** - `MADV_HUGEPAGE` para eficiência de TLB
- **Alocador mimalloc** - Fragmentação reduzida
- **Vetores quantizados** - i16 em vez de f64 para eficiência de cache

### CPU
- **AVX2 SIMD** - Cálculos de distância paralelos
- **Busy polling** - Reduz latência sob carga
- **Deferimento de label** - Pula buscas de subárvores desnecessárias
- **Saída antecipada** - Para busca quando resultado é confiante

### E/S
- **Epoll edge-triggered** - Notificação de eventos eficiente
- **Sockets não-bloqueantes** - `TCP_NODELAY`, modo não-bloqueante
- **Passagem de descritores de arquivo** - Zero-copy entre processos
- **Pooling de buffers** - Reutiliza buffers para reduzir alocações

## Formato do Índice

O índice usa um formato binário customizado (V5):

```
Header:
- Magic: "RNSPCST5" (8 bytes)
- Scale: i32
- Packed dimensions: i32
- Reference count: i32
- Partition count: i32
- Node count: i32
- Block count: i32
- Partition scheme: i16 (scheme_id, param, cut counts)
- Tree predicates: [dim: u8, flags: u8, threshold: i16]

Seções de dados:
- Partitions: [key: u16, root: u32, min: [i16; 16], max: [i16; 16]]
- Nodes: [left: i32, right: i32, start: u32, len: u16]
- Vectors: [i16; 16] blocks (alinhados com AVX2)
- Labels: [u8; 8] por block
- Reference indices: [u32; 8] por block
- Node class bits: [u8] para deferimento de label
```

## Licença

MIT
