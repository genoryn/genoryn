# Translation glossary

The website and the wallet use these words in every language. Translations started as machine
translations (marked `"_review": "machine"` in each catalog) and are waiting for people who
speak the language; corrections are welcome as pull requests ([CONTRIBUTING.md](../../CONTRIBUTING.md)).

## Never translated

Keep exactly as written, in every language:

- Names: **Genoryn**, **Genoryn Krypton**, **GNR**, **Genoryn Talks**, **Quick Swap**, **KryptoHash**, **Zarazi Sokani**, **Hive OS**, **GitHub**.
- Cryptography and standards: zk-STARK, ML-KEM, ML-KEM-1024, ML-DSA, SLH-DSA, X25519, BLAKE3, SHA-256, SHA3, FIPS 203, NIST, Winterfell, Rescue-Prime, XChaCha20-Poly1305, LWMA.
- Units and numbers: GNR, BTC, sat, MH/s, GH/s, H/s, GiB, MiB, GB, KB, ms. Write numbers as the English does (`1,000`, `0.5`): the build turns them into your language's style (`1.000` and `0,5` in German, `1 000` in French), and live values are formatted by the browser.
- Anything in code: commands, flags (`--node`), file names, paths, URLs, API paths (`/v1/info`), addresses (`gnr1…`, `gnrs1…`), payment proofs (`gnrpay1…`), swap codes (`gks1…`), hashes, keys.
- Recovery-phrase words and the wordlist, in any place they appear.
- Hardware and software names: NVIDIA, AMD, Intel, CUDA, OpenCL, RTX 5090, Windows, Linux, macOS, Bitcoin Core, Electrum, Esplora.

## Terms

| English | de | fr | es | pt | ru | tr | zh-CN | ja | ko |
|---|---|---|---|---|---|---|---|---|---|
| wallet | Wallet | portefeuille | billetera | carteira | кошелёк | cüzdan | 钱包 | ウォレット | 지갑 |
| light wallet | Light-Wallet | portefeuille léger | billetera ligera | carteira leve | лёгкий кошелёк | hafif cüzdan | 轻钱包 | ライトウォレット | 라이트 지갑 |
| mining | Mining | minage | minería | mineração | майнинг | madencilik | 挖矿 | マイニング | 채굴 |
| miner (program) | Miner | mineur | minero | minerador | майнер | madenci | 挖矿程序 | マイナー | 채굴기 |
| node | Node | nœud | nodo | nó | узел | düğüm | 节点 | ノード | 노드 |
| full node | Full Node | nœud complet | nodo completo | nó completo | полный узел | tam düğüm | 全节点 | フルノード | 풀 노드 |
| block | Block | bloc | bloque | bloco | блок | blok | 区块 | ブロック | 블록 |
| share (mining) | Share | share | share | share | шара | share | 份额 | シェア | 셰어 |
| hashrate | Hashrate | hashrate | hashrate | hashrate | хешрейт | hashrate | 算力 | ハッシュレート | 해시레이트 |
| reward | Belohnung | récompense | recompensa | recompensa | вознаграждение | ödül | 奖励 | 報酬 | 보상 |
| payout | Auszahlung | paiement | pago | pagamento | выплата | ödeme | 发放 | 支払い | 지급 |
| fee | Gebühr | frais | comisión | taxa | комиссия | ücret | 手续费 | 手数料 | 수수료 |
| recovery phrase | Wiederherstellungsphrase | phrase de récupération | frase de recuperación | frase de recuperação | фраза восстановления | kurtarma ifadesi | 助记词 | リカバリーフレーズ | 복구 문구 |
| password | Passwort | mot de passe | contraseña | senha | пароль | parola | 密码 | パスワード | 비밀번호 |
| address | Adresse | adresse | dirección | endereço | адрес | adres | 地址 | アドレス | 주소 |
| short address | Kurzadresse | adresse courte | dirección corta | endereço curto | короткий адрес | kısa adres | 短地址 | 短縮アドレス | 짧은 주소 |
| atomic swap | Atomic Swap | échange atomique | intercambio atómico | troca atômica | атомарный своп | atomik takas | 原子交换 | アトミックスワップ | 아토믹 스왑 |
| swap | Swap | échange | intercambio | troca | своп | takas | 兑换 | スワップ | 스왑 |
| offer | Angebot | offre | oferta | oferta | предложение | teklif | 挂单 | オファー | 오퍼 |
| offer board | Angebotstafel | tableau des offres | tablón de ofertas | quadro de ofertas | доска предложений | teklif panosu | 挂单板 | オファーボード | 오퍼 보드 |
| market price | Marktpreis | prix du marché | precio de mercado | preço de mercado | рыночная цена | piyasa fiyatı | 市场价格 | 市場価格 | 시장 가격 |
| explorer | Explorer | explorateur | explorador | explorador | обозреватель блоков | blok gezgini | 区块浏览器 | エクスプローラー | 익스플로러 |
| quantum-resistant | quantenresistent | résistant aux ordinateurs quantiques | resistente a la computación cuántica | resistente à computação quântica | квантово-устойчивый | kuantuma dayanıklı | 抗量子 | 耐量子 | 양자 내성 |
| shielded (transfer) | abgeschirmt | protégé | protegido | protegido | скрытый | korumalı | 屏蔽 | シールド | 보호된 |
| quantum tokens | Quantum-Token | jetons quantiques | tokens cuánticos | tokens quânticos | квантовые токены | kuantum tokenleri | 量子代币 | クォンタムトークン | 퀀텀 토큰 |
| fair launch | Fair Launch | lancement équitable | lanzamiento justo | lançamento justo | честный запуск | adil başlangıç | 公平启动 | フェアローンチ | 공정 출시 |
| premine | Premine | prémine | preminado | pré-mineração | премайн | ön madencilik | 预挖 | プレマイン | 사전 채굴 |

## Style

| Language | Address the reader as | Notes |
|---|---|---|
| de | du | Short sentences; nouns capitalised; "Bitcoin" as in English |
| fr | vous | Space before `: ; ? !` as French typography wants |
| es | tú | Neutral Spanish, no regional slang |
| pt | você | Brazilian Portuguese |
| ru | вы | lowercase «вы»; quotes «…» |
| tr | siz | |
| zh-CN | 你 | Simplified Chinese; full-width punctuation |
| ja | です・ます | Full-width punctuation |
| ko | 합니다 / 해요 as fits | |

Warnings about recovery phrases, passwords, scams and money are translated exactly, never
softened or shortened. Legal sentences ("not investment advice") likewise.
