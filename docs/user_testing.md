# Roteiro de teste com usuários

Roteiro para validação do sistema de busca em documentação interna com usuários reais.

---

## 1. Pré-teste

### 1.1 Setup do ambiente

- [ ] **Acesso ao servidor**
  - URL do servidor (ex.: `http://localhost:8080` ou ambiente de homologação)
  - Navegador recomendado: Chrome ou Firefox (última versão)
- [ ] **Credenciais** (se aplicável)
  - Login/senha ou token de acesso (fornecer antes da sessão)
  - Instruções de primeiro acesso (ex.: reset de senha)
- [ ] **Checklist técnico**
  - Servidor rodando e acessível
  - Dashboard e endpoint de busca respondendo
  - Coleção de documentos indexada e disponível para busca

### 1.2 Escopo explicado ao participante

> **O que vamos testar:** um **sistema de busca em documentação interna**.  
> Você poderá fazer perguntas em linguagem natural e o sistema retornará trechos relevantes dos nossos documentos (README, API, arquitetura, exemplos, etc.).  
> O objetivo é avaliar se as respostas ajudam no dia a dia e onde o sistema pode melhorar.

### 1.3 Duração

- **Tempo total sugerido:** 2–3 horas de uso real
- **Estrutura:** ~30 min de tarefas guiadas + 1–2 h de uso livre + ~15 min de entrevista/feedback

---

## 2. Tarefas guiadas (primeiros ~30 min)

Oriente o participante: _“Nas próximas tarefas, use o sistema de busca como faria no trabalho. Não há resposta certa ou errada; queremos ver como você usa e o que encontra.”_

### Tarefa 1

**Objetivo:** “Encontre como fazer deploy da aplicação.”

- O participante deve usar a busca para descobrir passos ou referências a deploy.
- **Observar:** termos usados na query, se refinou a pergunta, se ficou satisfeito com o resultado.

### Tarefa 2

**Objetivo:** “Descubra quais são as variáveis de ambiente necessárias.”

- O participante deve localizar documentação sobre variáveis de ambiente (ex.: `.env`, configuração).
- **Observar:** se encontrou lista ou referência clara, se precisou de mais de uma busca.

### Tarefa 3

**Objetivo:** “Ache informação sobre troubleshooting de erro X.”

- Substitua _“erro X”_ por um erro real da base de docs (ex.: erro de conexão, timeout, falha na ingestão).
- **Observar:** se a busca retornou algo útil para diagnóstico ou resolução.

**Anotações do facilitador (durante as tarefas):**

- Queries digitadas (ou screenshot)
- Se completou a tarefa com sucesso / parcialmente / não
- Comentários espontâneos do participante

---

## 3. Uso livre (1–2 h)

- **Instrução ao participante:**  
  _“Use o sistema para suas dúvidas reais do dia a dia, como faria com a documentação. Pode ser sobre deploy, API, exemplos, configuração, erros, etc.”_

- **Auto-registro (participante):**
  - Para cada dúvida/pergunta que fizer:
    - **Resposta útil** → anotar brevemente o que ajudou
    - **Resposta não útil** → anotar o que faltou ou o que estava errado/confuso

- **Formato sugerido para anotação:**

  | Pergunta que fiz | Útil? (sim/não) | Observação breve |
  | ---------------- | --------------- | ---------------- |
  | ...              | ...             | ...              |

- O facilitador pode acompanhar em tempo real (compartilhamento de tela) ou combinar envio de anotações ao final.

---

## 4. Coleta de feedback

### 4.1 Formulário estruturado (ex.: Google Forms)

**Bloco 1 – Escalas (1–5)**

- **Facilidade de uso**  
  “Quão fácil foi usar o sistema de busca?”  
  (1 = Muito difícil … 5 = Muito fácil)

- **Qualidade das respostas**  
  “Quão satisfeito você ficou com a qualidade das respostas?”  
  (1 = Muito insatisfeito … 5 = Muito satisfeito)

- **Velocidade**  
  “Quão adequada foi a velocidade de resposta do sistema?”  
  (1 = Muito lenta … 5 = Muito rápida)

**Bloco 2 – Perguntas abertas**

- **O que funcionou bem?**  
  (texto livre)

- **O que não funcionou?**  
  (texto livre)

- **O que você esperava que tivesse mas não tem?**  
  (texto livre)

**Sugestão:** enviar o link do formulário logo após a sessão (ou preencher junto na entrevista).

---

### 4.2 Entrevista curta (~15 min)

- **Formato:** remota com compartilhamento de tela (ou presencial com um dispositivo do participante).
- **Foco:** observar uso real e aprofundar em falhas e expectativas.

**Roteiro sugerido:**

1. **Uso real (5 min)**
   - “Mostre como você usaria o sistema para uma dúvida que você teve esta semana.”
   - Observar: termos da busca, como interpreta os resultados, se desiste ou refina.

2. **Queries que falharam (5 min)**
   - “Teve alguma pergunta em que o sistema não ajudou ou deu resposta estranha? Pode mostrar ou descrever.”
   - Anotar: query exata (ou aproximada), o que o sistema retornou, o que o participante esperava.

3. **Fechamento (5 min)**
   - “Se você pudesse pedir uma melhoria, qual seria a primeira?”
   - “Alguma coisa que gostaria de acrescentar sobre a experiência?”

**Checklist do facilitador:**

- [ ] Gravação/consentimento (se aplicável)
- [ ] Anotações ou transcrição das queries problemáticas
- [ ] Resumo: 3 pontos positivos e 3 pontos a melhorar

---

## Resumo da sessão (template)

| Item                               | Preenchimento       |
| ---------------------------------- | ------------------- |
| Data                               |                     |
| Participante (ou perfil)           |                     |
| Duração real                       |                     |
| Tarefas 1–3: concluídas?           | Sim / Parcial / Não |
| Nº de buscas no uso livre (aprox.) |                     |
| Principais queixas                 |                     |
| Principais elogios                 |                     |
| Próximos passos (melhorias)        |                     |

---

_Documento de apoio ao teste com usuários do sistema de busca em documentação interna (ferres-db-core)._
