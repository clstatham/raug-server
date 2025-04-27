use anyhow::Result;
use raug::prelude::*;
use rosc::OscPacket;

use crate::graph::{GraphOp, GraphOpResponse};

pub struct Server {
    graph: Graph,
    running_graph: Option<RunningGraph>,
    mixer: Vec<Node>,
    master: Node,
    backend: AudioBackend,
    device: AudioDevice,
}

impl Server {
    pub fn new(inputs: usize, outputs: usize, backend: AudioBackend, device: AudioDevice) -> Self {
        let graph = Graph::new(inputs, outputs);

        let mixer = vec![
            Passthrough::<f32>::default().node(&graph, ()),
            Passthrough::<f32>::default().node(&graph, ()),
        ];
        let master = &mixer[0] + &mixer[1];

        graph.dac((&master, &master));

        Self {
            graph,
            running_graph: None,
            mixer,
            master,
            backend,
            device,
        }
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn num_mixer_channels(&self) -> usize {
        self.mixer.len()
    }

    pub fn mixer_channel(&mut self, index: usize) -> &Input {
        if index < self.num_mixer_channels() {
            self.mixer[index].input(0)
        } else {
            let mut i = index;
            while i >= self.num_mixer_channels() {
                let channel = Passthrough::<f32>::default().node(&self.graph, ());
                self.master = &self.master + &channel;
                self.mixer.push(channel);
                i -= 1;
            }
            self.mixer[index].input(0)
        }
    }

    pub fn start_graph(&mut self) -> Result<()> {
        let graph = self
            .graph
            .play(CpalOut::spawn(&self.backend, &self.device))?;
        self.running_graph = Some(graph);
        Ok(())
    }

    pub fn stop_graph(&mut self) -> Result<()> {
        if let Some(graph) = self.running_graph.take() {
            graph.stop()?;
        }
        Ok(())
    }

    pub fn apply_osc(&mut self, packet: &OscPacket) -> Result<Vec<GraphOpResponse>> {
        let mut responses = vec![];

        let ops = GraphOp::try_from_osc(packet)?;

        for op in ops {
            responses.push(op.apply(self)?);
        }

        Ok(responses)
    }
}
