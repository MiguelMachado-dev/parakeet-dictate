# Mac: o que falta

O app está em `mac/` (como compilar e usar: `mac/README.md`). Ele transcreve em um de dois lugares, escolhido no menu:

- **Neste Mac**: Parakeet TDT 0.6B v3 (o mesmo modelo do PC) em Core ML pelo FluidAudio, e GLM direto do Mac com uma chave da Z.ai.
- **No PC**: manda o áudio para o **Server for Mac** do app do Windows, que transcreve e pós-processa.

Itens com [x] foram verificados. O resto está implementado e espera um teste de verdade, exceto o overlay, que ainda não começou.

## Já verificado (2026-10-12)

- [x] `swift test`: reducer portado com os testes de `tests/test_reducer.py`, regras do GLM de `tests/test_polish.py`, WAV, multipart, tons e reamostragem.
- [x] Modelo local pelo código do app (`--check-local`): `pt.wav` e `en.wav` em cerca de 50 ms cada. Diferença em relação a `pt.txt`: "tudo bem." em vez de "tudo bem?".
- [x] Primeira carga do modelo: download de 470 MB, mais cerca de 12 s de compilação para o Neural Engine. Depois disso, inclusive após um rebuild, carrega em menos de 0,5 s.
- [x] Permissões sobrevivem a um rebuild: o app reaberto escutou o atalho na hora, sem pedir Acessibilidade de novo.
- [x] Cliente do PC contra o código real do `server.py`, rodando no Mac com um modelo falso (`mac/scripts/fake_pc_server.py`): upload aceito, `postprocess` omitido, `true` e `false`, token errado e porta fechada.
- [x] `.app` assinado com certificado local estável; o requisito designado não muda entre builds.
- [x] App abre, pede Microfone e Acessibilidade, e começa a escutar o Command direito assim que a Acessibilidade é concedida, sem reiniciar.

## 1. Primeiro ditado neste Mac

- [ ] Segurar o Command direito: som de início. Falar, soltar: som de fim, e o texto aparece no editor seguido de espaço.
- [ ] O clipboard volta a ter o que tinha antes (testar com texto e com uma imagem copiada).
- [ ] Toque rápido: mãos livres até apertar de novo.
- [ ] Esc durante a gravação: som de cancelar, nada colado.
- [ ] Command direito + C: nada gravado, sem som.
- [ ] Ditado longo (1 min ou mais).

Se segurar o Command direito não fizer som nenhum, olhar o log (**Open Log** no menu). Sem a linha `Idle -> Recording`, trocar o atalho para **F13** em Settings: se o F13 funcionar, o problema está na detecção do Command direito. Se o Superwhisper estiver aberto com o mesmo atalho, fechar.

## 2. GLM neste Mac

- [ ] Colar a chave da Z.ai em Settings.
- [ ] `"mac/build/Parakeet Dictate.app/Contents/MacOS/ParakeetDictate" --check-glm "hum então a reunião é tipo sexta"` devolve o texto limpo.
- [ ] Ditado com **Post-process with GLM** ligado: texto polido colado.
- [ ] Esc durante o pós-processamento: cola o texto cru, como no Windows.

## 3. No PC (uma vez)

- [ ] Reiniciar o app do Windows (Quit no tray e abrir pelo ícone), para ter o item **Server for Mac**.
- [ ] Rodar `.\scripts\allow_mac.ps1` na pasta do projeto. Pede administrador. Abre a porta 8765 só para a rede local.
- [ ] Marcar **Server for Mac** no tray. Se o Windows perguntar sobre o Python no firewall, clicar **Permitir acesso** com Privada e Pública marcadas.
- [ ] Clicar **Copy Mac connection info** e mandar as duas linhas para o Mac. Se falhar, a URL é `http://192.168.3.2:8765` e o token está em `server_token.txt` na pasta do projeto.
- [ ] Opcional: reservar o IP `192.168.3.2` para o PC no roteador (reserva de DHCP), para a URL não mudar.

Em 2026-10-12 a porta 8765 do PC dava timeout a partir do Mac: este passo ainda não tinha sido feito.

## 4. Ditar pelo PC

- [ ] Em Settings, **Paste Connection Info** e depois **Test Connection**.
- [ ] `--check-server tests/fixtures/pt.wav` com o PC: o texto bate com `pt.txt`.
- [ ] Trocar para **Transcribe on the PC** no menu e ditar.
- [ ] Na primeira requisição, ver se o macOS pede permissão de Rede Local. O `Info.plist` já tem `NSLocalNetworkUsageDescription` e `NSAllowsLocalNetworking`; se a requisição falhar com erro de ATS ou de rede local, está no log.
- [ ] Esc durante a requisição: som de cancelar, nada colado.
- [ ] Desligar o **Server for Mac**: som de erro, nada colado, o menu mostra o motivo.
- [ ] Ditar no PC e no Mac ao mesmo tempo: os dois funcionam (um espera o outro).

| Sintoma | Causa provável |
| --- | --- |
| Timeout | Regra de firewall faltando, IP errado, NordVPN escondendo o PC da rede local, ou Mac em outra rede |
| Connection refused | **Server for Mac** desligado no tray, ou o app do Windows fechado |
| 401 | Token errado ou faltando |
| 503 | O PC ainda está carregando o modelo; tentar de novo em alguns segundos |

## 5. Overlay

- [ ] Aparece embaixo, no centro da tela da janela em foco, 250 ms depois de apertar o atalho, e segue a voz.
- [ ] Atalho (Command direito + C): o overlay nunca aparece.
- [ ] Fim: o check verde ao colar; cinza no Esc; vermelho no erro.
- [ ] Não rouba o foco nem cliques, e aparece sobre apps em tela cheia e em todos os Spaces.
- [ ] Trocar entre Fluid, Bars e Off no menu.
- [ ] Em Top, aparece logo abaixo da barra de menu (e do notch), centralizado; trocar para Bottom no menu volta para baixo na ditada seguinte.

## 6. Pausar a mídia

Verificado no macOS 27.0.1 com o `--check-media` e um player de teste: um atalho de 0,1 s não pausa nada, uma gravação pausa, e parar retoma.

- [ ] Com música ou um vídeo tocando, segurar o atalho pausa quando o overlay aparece, e soltar retoma.
- [ ] Com tudo pausado, ditar não inicia nada.
- [ ] Quit durante uma gravação retoma o que pausou.

## 7. Regras por app

Verificado em 2026-10-12 com o `--check-glm` e a chave da Z.ai: com `Translate the transcript to English.`, frases em português saem em inglês, em geral já sem a parte corrigida ("sexta, não, quinta" vira "Thursday"); sem regra, nada muda. Pelo PC, com o `fake_pc_server.py`: o campo `instructions` chega ao GLM do servidor.

- [ ] **App Rules…** no menu: **+** > Translate to English, **Add App** > Slack.
- [ ] Ditar em português no Slack: cola em inglês. No log, a linha `rules for Slack: Translate to English`.
- [ ] Ditar em outro app: cola em português, como antes.
- [ ] Com **Post-process** desligado, a regra ainda traduz no Slack.
- [ ] Com o provedor Anthropic (Claude) em Settings, a mesma tradução no Slack.
- [ ] Pelo PC, depois de atualizar e reiniciar o app do Windows: a mesma tradução no Slack.

## 8. Depois

- [ ] Instalar em `/Applications` com `mac/scripts/build-app.sh --install` e ligar **Open at Login** no menu.
- [ ] Comparar o Parakeet Ultra (`AsrModelVersion.ultra` no FluidAudio, mesmo tamanho, mais preciso segundo o FluidAudio) com o v3 em português.

## 9. Reuniões (fase 1)

Transcrição manual pelo menu (**Start Meeting Transcription**), em duas faixas (microfone = `Me`, saída de áudio do Mac = `Others`), para um `.md` em `~/Documents/Parakeet/Meetings`. Ainda sem deteção automática; essa é a fase 2. Detalhes na secção Meetings de `mac/README.md`. Implementado sem poder compilar no Windows: o primeiro passo é mesmo ver se compila.

- [ ] `swift test --package-path mac`: passam também os novos `SpeechSegmenterTests` e `MeetingTranscriptTests`.
- [ ] `mac/scripts/build-app.sh --install` compila e instala sem erros.
- [ ] `open -W -n --stdout /dev/stdout "mac/build/Parakeet Dictate.app" --args --check-meeting 20`: falar e pôr um vídeo com fala a tocar ao mesmo tempo. Na primeira vez o macOS pede a permissão de áudio do sistema; aceitar. No fim imprime o `.md` com linhas `Me:` (a tua voz) e `Others:` (o vídeo).
- [ ] Se `Others` vier vazio e o log disser `peak levels Me ..., Others 0`, a permissão ficou por dar: Definições do Sistema > Privacidade e Segurança > Gravação de ecrã e áudio do sistema, ligar o Parakeet Dictate, e repetir o passo anterior.
- [ ] Reunião a sério no Zoom, com auscultadores: **Start Meeting Transcription** antes de entrar, **Stop** no fim (o menu mostra o tempo decorrido). O ficheiro abre sozinho; as falas e os tempos batem. Incluir pausas longas, falas curtas ("ok", "sim") e gente a falar por cima, para ver os cortes do segmentador.
- [ ] O mesmo no Meet, no browser: os outros entram em `Others`. Um vídeo a tocar noutra tab ao mesmo tempo também entra: é a limitação conhecida da fase 1, só confirmar que é isso e não outra coisa.
- [ ] Sem auscultadores, com colunas: os outros aparecem nas duas faixas. Esperado; só confirmar que nada parte.
- [ ] Com **Transcribe on the PC** escolhido, **Start Meeting Transcription** recusa com um alerta a explicar. Durante uma reunião a decorrer, trocar para o PC no menu também recusa.
- [ ] Ditar com o Command direito durante a reunião: cola como sempre, e a fala também fica na faixa `Me`.
- [ ] Quit a meio de uma reunião: o `.md` fica fechado, com rodapé e tudo o que já tinha sido transcrito.
- [ ] Reunião longa (30 min ou mais): abrir o `.md` a meio para confirmar que vai crescendo, e no fim ver o log sem segmentos "failed".

Se os cortes das falas não agradarem (frases partidas a meio ou juntas demais), os knobs estão nos defaults de `SpeechSegmenter` em `mac/Sources/DictateCore/SpeechSegmenter.swift`: `silenceSeconds` (pausa para fechar uma fala), `speechRMS` (o que conta como fala) e `maxSegmentSeconds`.
