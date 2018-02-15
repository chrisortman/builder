// Copyright (c) 2016-2017 Chef Software Inc. and/or applicable contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { List, Record } from 'immutable';
import * as actionTypes from '../actions/index';
import initialState from '../initial-state';

export default function builds(state = initialState['builds'], action) {
  switch (action.type) {

    case actionTypes.CLEAR_BUILD:
      return state
        .setIn(['selected', 'info'], Record({})())
        .setIn(['ui', 'selected', 'info', 'loading'], false);

    case actionTypes.CLEAR_BUILD_LOG:
      state.get('selected').log.content.next([]);

      return state
        .setIn(['selected', 'log'], {
          start: undefined,
          stop: undefined,
          content: state.get('selected').log.content,
          is_complete: undefined
        })
        .setIn(['ui', 'selected', 'log', 'loading'], false);

    case actionTypes.CLEAR_BUILDS:
      return state
        .setIn(['visible'], List())
        .setIn(['ui', 'loading'], false);

    case actionTypes.POPULATE_BUILD:
      return state.setIn(['selected', 'info'], action.payload);

    case actionTypes.POPULATE_BUILD_LOG:

      if (action.error) {
        return state.setIn(['selected', 'log'], {
          start: undefined,
          stop: undefined,
          content: state.get('selected').log.content,
          is_complete: undefined
        });
      }
      else {
        let payload = action.payload;
        let content = state.get('selected').log.content;

        if (payload.start === 0 && !payload.is_complete) {
          content.next(payload.content || []);
        }
        else if (payload.content.length) {
          content.next(payload.content);
        }

        return state.setIn(['selected', 'log'], {
          start: payload.start,
          stop: payload.stop,
          content: content,
          is_complete: payload.is_complete
        });
      }

    case actionTypes.POPULATE_BUILDS:
      return state.setIn(['visible'], List(action.payload));

    case actionTypes.STREAM_BUILD_LOG:
      return state.setIn(['selected', 'stream'], action.payload);

    case actionTypes.SET_BUILD_LOADING:
      return state.setIn(['ui', 'selected', 'info', 'loading'], action.payload);

    case actionTypes.SET_BUILDS_LOADING:
      return state.setIn(['ui', 'loading'], action.payload);

    case actionTypes.SET_BUILD_LOG_LOADING:
      return state.setIn(['ui', 'selected', 'log', 'loading'], action.payload);

    case actionTypes.SET_BUILD_LOG_NOT_FOUND:
      return state.setIn(['ui', 'selected', 'log', 'notFound'], action.payload);

    default:
      return state;
  }
}
