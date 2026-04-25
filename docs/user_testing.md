# User Testing Script

Script for validating the internal documentation search system with real users.

---

## 1. Pre-test

### 1.1 Environment setup

- [ ] **Server access**
  - Server URL (e.g. `http://localhost:8080` or staging environment)
  - Recommended browser: Chrome or Firefox (latest version)
- [ ] **Credentials** (if applicable)
  - Login/password or access token (provide before the session)
  - First-access instructions (e.g. password reset)
- [ ] **Technical checklist**
  - Server running and accessible
  - Dashboard and search endpoint responding
  - Document collection indexed and available for search

### 1.2 Scope explained to the participant

> **What we are testing:** an **internal documentation search system**.  
> You will be able to ask questions in natural language and the system will return relevant excerpts from our documents (README, API, architecture, examples, etc.).  
> The goal is to evaluate whether the responses help in day-to-day work and where the system can be improved.

### 1.3 Duration

- **Suggested total time:** 2–3 hours of actual use
- **Structure:** ~30 min of guided tasks + 1–2 h of free use + ~15 min interview/feedback

---

## 2. Guided tasks (first ~30 min)

Guide the participant: _"In the following tasks, use the search system as you would at work. There is no right or wrong answer; we want to see how you use it and what you find."_

### Task 1

**Goal:** "Find out how to deploy the application."

- The participant should use the search to discover steps or references to deployment.
- **Observe:** terms used in the query, whether they refined the question, whether they were satisfied with the result.

### Task 2

**Goal:** "Find out what environment variables are required."

- The participant should locate documentation about environment variables (e.g. `.env`, configuration).
- **Observe:** whether they found a clear list or reference, whether they needed more than one search.

### Task 3

**Goal:** "Find information about troubleshooting error X."

- Replace _"error X"_ with a real error from the docs base (e.g. connection error, timeout, ingestion failure).
- **Observe:** whether the search returned something useful for diagnosis or resolution.

**Facilitator notes (during tasks):**

- Queries typed (or screenshot)
- Whether the task was completed successfully / partially / not at all
- Participant's spontaneous comments

---

## 3. Free use (1–2 h)

- **Instruction to participant:**  
  _"Use the system for your real day-to-day questions, as you would with documentation. It can be about deployment, API, examples, configuration, errors, etc."_

- **Self-registration (participant):**
  - For each question/query they make:
    - **Useful response** → briefly note what helped
    - **Not useful response** → note what was missing or what was wrong/confusing

- **Suggested annotation format:**

  | Question asked | Useful? (yes/no) | Brief observation |
  | -------------- | ---------------- | ----------------- |
  | ...            | ...              | ...               |

- The facilitator can follow in real time (screen sharing) or arrange to receive notes at the end.

---

## 4. Feedback collection

### 4.1 Structured form (e.g. Google Forms)

**Block 1 – Scales (1–5)**

- **Ease of use**  
  "How easy was it to use the search system?"  
  (1 = Very difficult … 5 = Very easy)

- **Response quality**  
  "How satisfied were you with the quality of the responses?"  
  (1 = Very unsatisfied … 5 = Very satisfied)

- **Speed**  
  "How adequate was the system's response speed?"  
  (1 = Very slow … 5 = Very fast)

**Block 2 – Open questions**

- **What worked well?**  
  (free text)

- **What didn't work?**  
  (free text)

- **What did you expect to be there but wasn't?**  
  (free text)

**Suggestion:** send the form link right after the session (or fill it in together during the interview).

---

### 4.2 Short interview (~15 min)

- **Format:** remote with screen sharing (or in-person with the participant's device).
- **Focus:** observe real usage and dig deeper into failures and expectations.

**Suggested script:**

1. **Real use (5 min)**
   - "Show me how you would use the system for a question you had this week."
   - Observe: search terms, how they interpret the results, whether they give up or refine.

2. **Failed queries (5 min)**
   - "Was there any question where the system didn't help or gave a strange response? Can you show or describe it?"
   - Note: exact (or approximate) query, what the system returned, what the participant expected.

3. **Closing (5 min)**
   - "If you could request one improvement, what would it be?"
   - "Is there anything you'd like to add about the experience?"

**Facilitator checklist:**

- [ ] Recording/consent (if applicable)
- [ ] Notes or transcript of problematic queries
- [ ] Summary: 3 positive points and 3 points for improvement

---

## Session summary (template)

| Item                                  | Entry               |
| ------------------------------------- | ------------------- |
| Date                                  |                     |
| Participant (or profile)              |                     |
| Actual duration                       |                     |
| Tasks 1–3: completed?                 | Yes / Partial / No  |
| Number of searches in free use (approx.) |                  |
| Main complaints                       |                     |
| Main praise                           |                     |
| Next steps (improvements)             |                     |

---

_Supporting document for user testing of the internal documentation search system (ferres-db-core)._
