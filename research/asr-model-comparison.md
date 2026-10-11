# Pesquisa: alternativas locais ao Parakeet para ditado pt-BR

## Resumo

**Recomendação: manter o Parakeet atual como padrão; experimentar TAGARELA como opção específica de português, não como substituição garantida.** Whisper large-v3-turbo é o controle alternativo mais útil; Qwen3-ASR-0.6B merece avaliação exploratória, mas não demonstrou superioridade em pt-BR nas fontes examinadas. Não há comparação reproduzível comum de todos esses modelos para clipes curtos de push-to-talk.

Escopo: fontes primárias disponíveis, sem presumir lançamentos pela data de referência do projeto (2026-10-10). Nenhum modelo foi baixado/executado, nem áudio local enviado. Contexto recebido do orquestrador, não medido nesta pesquisa: RTX 5080/16 GB; sete transcrições históricas levaram 54–168 ms no Parakeet e 1.465–5.899 ms no pós-processamento GLM. **Esses registros não são benchmark**, mas sugerem que trocar ASR por velocidade não ataca o principal atraso.

## Achados

### 1. Candidatos e tamanho real

**Suporte: evidência direta nas fontes indicadas; confiança alta para contagens oficiais, média para declarações de fine-tune.** “CUDA disponível” não significa integração validada no Windows 11/CUDA 13/ONNX Runtime do aplicativo.

| Candidato | Parâmetros | Inglês / português | Inferência local documentada e prioridade |
|---|---:|---|---|
| **Parakeet v3 pt-BR TAGARELA ONNX** | Classe 0,6B herdada; sem recontagem independente | Especializado em pt-BR; retenção de inglês **não garantida** | Card documenta `onnx-asr`; primeiro teste para precisão PT [T] |
| **Whisper large-v3-turbo** | **809M** | Multilíngue, incluindo EN/PT | PyTorch/Transformers CUDA; principal controle alternativo [W, WT] |
| **Whisper medium**, não `medium.en` | **769M** | Multilíngue | Família Whisper CUDA; controle secundário, não preferência inicial [W] |
| **Qwen3-ASR-0.6B** | **938.008.576**, aproximadamente **0,94B** | EN/PT entre 30 línguas; não promessa específica de sotaques brasileiros | Transformers e vLLM CUDA documentados; integração distinta [Q, QA] |
| **Canary-1b-v2** | **978M** | EN/PT entre 25 línguas; treinamento português europeu | NeMo/NVIDIA GPU; menor prioridade para pt-BR [C] |

**Qwen: o nome engana na comparação de porte.** A API oficial informa `safetensors.total=938008576`, todos BF16, no SHA `5eb144179a02acc5e5ba31e748d22b0cf3e303b0` [QA]. O relatório §2.1 descreve Qwen3-0.6B + encoder AuT de 180M + projetor; portanto, não basta tratar “0.6B” como total nem somar rótulos arredondados. A contagem acima exclui o ForcedAligner separado, desnecessário para simples texto.

O turbo reduz o decoder do large-v3 de 32 para quatro camadas [WT, introdução]. **Não confundir resultados do large-v3 completo (1.550M) com turbo ou medium.** Como triagem adicional, Fun-ASR-MLT-Nano-2512 declara aproximadamente 800M, EN/PT e CUDA, mas seu próprio card avisa que as tabelas da família **não são resultados específicos desse checkpoint** [F, “Performance”]. Sem WER português verificável, fica fora da lista prioritária.

### 2. O que realmente sabemos sobre WER português

**Qwen — evidência direta, confiança alta na transcrição dos números; média na transferência ao usuário.** O relatório oficial, **Tabela A.2(a–b)** [QR], publica:

| Dataset, linha `pt` | Qwen3-ASR-0.6B: WER (%) |
|---|---:|
| MLS | 12,16 |
| CommonVoice | 11,30 |
| MLC-SLM | 34,97 |
| FLEURS | 6,21 |

MLS/CommonVoice/FLEURS não representam diretamente ditado espontâneo pessoal; MLC-SLM é avaliação de fala conversacional (§4.1). O card Qwen, “Evaluation”, declara BF16, greedy, vLLM, `max_new_tokens=1024`, sem idioma forçado [Q]. Isso não equivale ao atual FP32 ONNX, nem a quantizações comunitárias.

O orquestrador verificou no [card NVIDIA](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), tabelas por idioma, Parakeet **FLEURS pt_br 4,76%; MLS 7,50%; CoVoST 3,96%**, greedy sem pontuação/caixa. **Interpretação, confiança média:** os números não sustentam trocar para Qwen esperando ganho português. Não são confronto controlado: normalização, versões, recortes e detecção de idioma podem diferir. Não comparar CommonVoice com CoVoST. Tampouco médias multilíngues ou rankings ingleses provam vantagem em pt-BR.

**TAGARELA — evidência direta do publicador do export, confiança média/baixa na generalização.** O card [T], “Dataset-level benchmark results”, apresenta confronto na mesma tabela. Abaixo, razões WER convertidas para porcentagem:

| Conjunto | Parakeet v3 original | TAGARELA v3 ONNX |
|---|---:|---:|
| Common Voice 21.0 | 8,1% | **5,1%** |
| MLS português | **6,4%** | 10,8% |
| MTEDx português | 18,6% | **13,3%** |
| Média preparada publicada | 9,0% | **7,5%** |
| Média espontânea publicada | 21,8% | **14,3%** |

A média espontânea inclui ALIP, C-ORAL Brasil I, NURC-Recife, SP2010, NURC-SP, MuPe e **um dataset privado**. A tabela também inclui Whisper large-v3, mas **não turbo, medium ou Qwen 0.6B**. Faltam no card protocolo completo, revisões dos modelos/dados, normalizador, decoding, hardware e intervalos de confiança. O checkpoint de treinamento vinculado respondeu **HTTP 401**; não foi possível auditar sua receita. Portanto, ganho promissor, não validação independente nem prova de ausência de contaminação.

O artigo **TAGARELA §4.2/Tabela 1** [TP] é outra experiência: fine-tune **Parakeet v2**, WER **15,18%**, versus v3 sem ajuste **23,30%** e Whisper large-v3 **20,91%**, no teste TAGARELA transcrito manualmente e normalizado. GPUs A100 ou B200. **Não atribuir 15,18% ao export v3.** O corpus de podcasts contém majoritariamente português brasileiro (§2), mas conversa de podcast continua diferente de comandos, números e nomes próprios ditados.

### 3. Velocidade, quantização e viabilidade

**Evidência direta; confiança alta sobre limites das medições.** Qwen §2.4/Tabela 2 [QR] usa clipes de aproximadamente **dois minutos**, vLLM 0.14.0, CUDA Graph e BF16. Os **92 ms são TTFT**, não tempo até texto completo; throughput 2.000× ocorre com **128 solicitações simultâneas**. O hardware é descrito apenas como “single typical computing resource”, insuficiente para extrapolar à RTX 5080. O README Whisper [W] mede velocidades relativas com **inglês em A100**; não permite prometer latência pt-BR por clipe.

Contexto de integração verificado pelo orquestrador: `onnx-asr` lista Parakeet/Canary/Whisper, não Qwen; TAGARELA publica FP32, **sem pesos int8 de fallback no mesmo repositório**, e a aplicação fixa o identificador do modelo. **Inferência:** TAGARELA tende a exigir menos mudança que Qwen, mas não é mera troca de pasta. Compatibilidade CUDA documentada dos modelos não certifica esses pacotes/versões locais. WER publicado não deve ser transferido para INT8 sem reavaliação.

## Contradições e lacunas

- TAGARELA melhora fala espontânea, mas piora MLS: especialização não é melhoria universal.
- “0.6B” Qwen versus 0,94B efetivos: rótulo comercial/arquitetural não é contagem completa.
- Não encontrado confronto confiável **turbo × medium × Qwen 0.6B × TAGARELA** em pt-BR sob protocolo único, nem retenção inglesa do fine-tune.
- `source_check` recuperou fontes, mas retornou `unclear` por indisponibilidade de avaliação semântica automática. Números acima foram conferidos manualmente no conteúdo original; upstream TAGARELA permaneceu inacessível, sem contorno de autenticação.

## Fontes mantidas e despriorizadas

- [Q — Card Qwen](https://huggingface.co/Qwen/Qwen3-ASR-0.6B): idiomas, CUDA e avaliação.
- [QA — API oficial HF](https://huggingface.co/api/models/Qwen/Qwen3-ASR-0.6B): contagem total dos tensores.
- [QR — Qwen Technical Report v2](https://arxiv.org/html/2601.21337v2): §§2.1/2.4/4.1 e Tabela A.2.
- [T — TAGARELA ONNX](https://huggingface.co/alefiury/parakeet-tdt-0.6b-v3-ptBR-TAGARELA-onnx): comparação PT e limitações.
- [TP — Artigo TAGARELA](https://arxiv.org/html/2603.15326v1): distinção entre fine-tune v2 e base v3.
- [W — OpenAI Whisper README](https://github.com/openai/whisper#available-models-and-languages), [WT — turbo](https://huggingface.co/openai/whisper-large-v3-turbo): parâmetros, arquitetura e CUDA.
- [C — Canary v2](https://huggingface.co/nvidia/canary-1b-v2): “Model Architecture” e “Evaluation Notes”.
- [F — Fun-ASR MLT](https://huggingface.co/FunAudioLLM/Fun-ASR-MLT-Nano-2512): triagem; ausência explícita de benchmark específico.

Despriorizados: comparadores SEO; números herdados de exports INT8 sem reavaliação; Qwen 1.7B e Whisper large-v3 como candidatos por excederem o porte solicitado. Foram usados somente como referências quando necessário.

## Próximos passos: decisão por A/B local

**Recomendação do pesquisador:** comparar primeiro base versus TAGARELA, depois turbo; Qwen e medium só se houver interesse/capacidade adicional. Usar os mesmos clipes curtos, transcrição humana e normalização fixa; separar pt-BR, inglês e mistura, incluindo números, siglas, termos técnicos, silêncio e ruído. Medir WER, preservação de nomes/números, pontuação e alucinações **antes do GLM**; depois medir resultado final, latência após soltar a tecla (p50/p95), carga inicial e VRAM. Testar FP32 CUDA e fallback INT8 separadamente. Adotar somente se o ganho PT compensar regressões inglesas e custo operacional; manter o original como reversão.
