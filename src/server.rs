use anyhow::Result;
use raug::{
    graph::{Graph, RunningGraph},
    prelude::CpalOut,
};
use rosc::OscPacket;

use raug_server::graph::{GraphOp, GraphOpResponse};

#[derive(Default)]
pub struct Server {
    graph: Graph,
    running_graph: Option<RunningGraph>,
}

impl Server {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_graph(&self) -> Result<RunningGraph> {
        let graph = self.graph.play(CpalOut::default())?;
        Ok(graph)
    }

    pub fn apply_osc(&mut self, packet: &OscPacket) -> Result<Vec<GraphOpResponse>> {
        let mut responses = vec![];

        let ops = GraphOp::try_from_osc(packet)?;

        for op in ops {
            match op {
                GraphOp::Play => {
                    let running_graph = self.graph.play(CpalOut::default())?;
                    self.running_graph = Some(running_graph);
                    responses.push(GraphOpResponse::None);
                }
                GraphOp::Stop => {
                    if let Some(running_graph) = self.running_graph.take() {
                        running_graph.stop()?;
                    }
                    responses.push(GraphOpResponse::None);
                }
                op => {
                    responses.push(op.apply(&self.graph)?);
                }
            }
        }

        Ok(responses)
    }
}
