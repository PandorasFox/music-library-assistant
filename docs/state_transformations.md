---
config:
  layout: elk
  theme: neutral
title: Witch state transformations diagram
---
flowchart BT
 subgraph TaskExecState["TaskExecutionState"]
        Work["Working"]
        Complete["Completed"]
        Idle["Idle"]
        
  end
 subgraph Tree["post-work-completion decision tree"]
        Tinvalid{"invalid op"}
        Troot{"is_observing?"}
        Tobs{"Eye State?"}
        Twork{"Eye State?"}
        TworkAwake{"mutations?"}
        TworkAwake2{"freshen latch?"}
  end
 subgraph Awakening["Awakening process"]
        AwakeningMutations["Eye → Awake"]
        AwakeningMutationsRO{"read only mode?"}
        AwakeningMutationsRW["accepting_mutations = TRUE"]
  end
    Work -- Finished! --> Troot
    Idle -- User Decision or idle observation of corpus --> Work
    Complete -. linger 30s .-> Idle
    Ccorpus["Queue ContentAnalysis"] -.-> Work
    Cobs["Queue Post-Observation Computations"] -.-> Work
    AwakeningMutations --> AwakeningMutationsRO
    AwakeningMutationsRO -- nope! --> AwakeningMutationsRW
    AwakeningMutationsRW -- Awake now! --> Complete
    AwakeningMutationsRO -. read-only mode .-> Idle
    Troot -. yes .-> Tobs
    Troot -. no .-> Twork
    Twork -. Asleep .-> Tinvalid
    Tobs -. Awakening .-> Tinvalid
    Tobs -. Awake or Asleep .-> Cobs
    Twork -- Awake --> TworkAwake
    Twork -- Awakening --> AwakeningMutations
    TworkAwake -- yes --> Ccorpus
    TworkAwake -- no --> TworkAwake2
    TworkAwake2 -- yes (clear latch) --> Ccorpus
    TworkAwake2 -- Nothing to do! --> Complete
